#!/usr/bin/env python3
"""Prepare only the user's existing Dolly/OASST conversation corpus for portable training."""

from __future__ import annotations

import argparse
import gzip
import hashlib
import json
import shutil
import sys
from collections import Counter
from pathlib import Path

import numpy as np
import pyarrow.parquet as pq

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "ml"))
from cortex_ml.circuit.tokenizer import Tokenizer, dialogue_tokens


def digest(path):
    h = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def canonical(text):
    return " ".join(text.casefold().split())


def prepare(root, output):
    root, output = Path(root), Path(output)
    output.mkdir(parents=True, exist_ok=True)
    provenance = output / "provenance"
    provenance.mkdir(exist_ok=True)
    sources, messages, heldout, candidates = [], {}, set(), []
    counts = Counter()
    for name in ("oasst1", "oasst2"):
        folder = root / name
        for path in sorted((folder / "raw/data").glob("*.parquet")):
            sources.append(
                {"path": str(path.relative_to(root)), "sha256": digest(path)}
            )
            for batch in pq.ParquetFile(path).iter_batches(batch_size=4096):
                for record in batch.to_pylist():
                    counts["raw_messages"] += 1
                    if "validation" in path.name:
                        heldout.add(record["message_tree_id"])
                    if record.get("deleted") or record.get("review_result") is False:
                        counts["deleted_or_rejected"] += 1
                        continue
                    record["source"] = name
                    # OASST releases overlap; retain one record per stable message ID.
                    if record["message_id"] in messages:
                        counts["duplicate_message_ids"] += 1
                    messages[record["message_id"]] = record
        for filename in (
            "manifest.json",
            "upstream_metadata.json",
            "raw/README.md",
            "raw/LICENSE",
        ):
            path = folder / filename
            if path.exists():
                shutil.copy2(path, provenance / f"{name}-{path.name}")
    for message in messages.values():
        if message["role"] != "assistant":
            continue
        chain, cursor, seen = [], message, set()
        while cursor is not None and cursor["message_id"] not in seen:
            seen.add(cursor["message_id"])
            chain.append(cursor)
            cursor = messages.get(cursor.get("parent_id"))
        chain.reverse()
        if not chain or chain[0].get("parent_id") is not None or len(chain) % 2:
            counts["incomplete_context"] += 1
            continue
        if any(
            item["role"] != ("prompter" if i % 2 == 0 else "assistant")
            or not isinstance(item.get("text"), str)
            or not item["text"].strip()
            for i, item in enumerate(chain)
        ):
            counts["invalid_turns"] += 1
            continue
        candidates.append(
            {
                "id": message["message_id"],
                "group": message["message_tree_id"],
                "source": message["source"],
                "language": message.get("lang", "unknown"),
                "upstream_validation": message["message_tree_id"] in heldout,
                "turns": [
                    {"question": chain[i]["text"], "answer": chain[i + 1]["text"]}
                    for i in range(0, len(chain), 2)
                ],
            }
        )
    folder = root / "dolly-15k"
    path = folder / "raw/databricks-dolly-15k.jsonl"
    sources.append({"path": str(path.relative_to(root)), "sha256": digest(path)})
    with path.open() as stream:
        for index, line in enumerate(stream):
            record = json.loads(line)
            question = record["instruction"]
            if record.get("context"):
                question += "\n\nContext:\n" + record["context"]
            if not question.strip() or not record["response"].strip():
                counts["invalid_turns"] += 1
                continue
            candidates.append(
                {
                    "id": f"dolly-{index}",
                    "group": f"dolly-{index}",
                    "source": "dolly-15k",
                    "language": "en",
                    "upstream_validation": False,
                    "turns": [{"question": question, "answer": record["response"]}],
                }
            )
    for filename in ("manifest.json", "upstream_metadata.json", "raw/README.md"):
        path = folder / filename
        if path.exists():
            shutil.copy2(path, provenance / f"dolly-15k-{path.name}")

    # Union tree groups sharing an exact normalized question at any turn. This
    # keeps OASST-release and cross-source duplicates on one side of the split.
    parent = {r["group"]: r["group"] for r in candidates}

    def find(group):
        while parent[group] != group:
            parent[group] = parent[parent[group]]
            group = parent[group]
        return group

    questions = {}
    for record in candidates:
        for turn in record["turns"]:
            key = hashlib.sha256(canonical(turn["question"]).encode()).hexdigest()
            previous = questions.setdefault(key, record["group"])
            a, b = sorted((find(previous), find(record["group"])))
            parent[b] = a
    test_groups = {find(r["group"]) for r in candidates if r["upstream_validation"]}
    splits, duplicates = {s: [] for s in ("train", "dev", "test")}, set()
    for record in candidates:
        group = find(record["group"])
        key = hashlib.sha256(
            json.dumps(record["turns"], sort_keys=True).encode()
        ).hexdigest()
        if key in duplicates:
            counts["duplicate_dialogues"] += 1
            continue
        duplicates.add(key)
        record["group"] = group
        hashed = int(hashlib.sha256(group.encode()).hexdigest()[:8], 16) % 100
        split = (
            "test"
            if group in test_groups or hashed < 2
            else "dev"
            if hashed < 5
            else "train"
        )
        record["split"] = split
        splits[split].append(record)
    if not all(splits.values()):
        raise ValueError("Corpus did not produce nonempty train/dev/test splits")
    print({s: len(rows) for s, rows in splits.items()}, flush=True)
    codec = Tokenizer.fit_fast(
        (
            turn[key]
            for row in splits["train"]
            for turn in row["turns"]
            for key in ("question", "answer")
        ),
        size=8192,
    )
    if len(codec.tokens) != 8192:
        raise ValueError(
            f"Corpus yielded only {len(codec.tokens)} vocabulary entries; 30M preset requires 8192"
        )
    (output / "tokenizer.json").write_text(json.dumps(codec.json()))
    stats = {}
    for split, rows in splits.items():
        index, offset = [], 0
        with (
            gzip.open(
                output / f"{split}.jsonl.gz", "wt", encoding="utf-8"
            ) as text_file,
            (output / f"{split}.tokens.bin").open("wb") as token_file,
            (output / f"{split}.targets.bin").open("wb") as target_file,
        ):
            for row in rows:
                sequence, targets = [], []
                for turn in row["turns"]:
                    seq, gold = dialogue_tokens(turn, codec)
                    sequence.extend(seq + [0])
                    targets.extend(gold + [-100])
                np.asarray(sequence, dtype="<u2").tofile(token_file)
                np.asarray(targets, dtype="<i2").tofile(target_file)
                index.append(
                    {
                        "id": row["id"],
                        "group": row["group"],
                        "source": row["source"],
                        "offset": offset,
                        "length": len(sequence),
                    }
                )
                offset += len(sequence)
                text_file.write(json.dumps(row, ensure_ascii=False) + "\n")
        (output / f"{split}.index.json").write_text(json.dumps(index))
        stats[split] = {
            "episodes": len(rows),
            "tokens": offset,
            "sources": dict(Counter(r["source"] for r in rows)),
            "languages": dict(Counter(r["language"] for r in rows)),
        }
        print(split, stats[split]["episodes"], offset, flush=True)
    files = [p for p in output.iterdir() if p.is_file() and p.name != "manifest.json"]
    manifest = {
        "version": 2,
        "source_root": str(root),
        "sources": sources,
        "counts": stats,
        "filters": dict(counts),
        "vocabulary": len(codec.tokens),
        "synthetic_examples_added": 0,
        "sequence_truncation": "none during preparation",
        "split_policy": "Connected groups of exact normalized questions stay together across sources/releases. Upstream validation groups remain test-only; hash assigns additional 2% test, 3% development. No test text trains the tokenizer.",
        "licenses": {
            "oasst1": "Apache-2.0",
            "oasst2": "Apache-2.0",
            "dolly-15k": "CC-BY-SA-3.0; see preserved source card",
        },
        "sha256": {p.name: digest(p) for p in files},
    }
    (output / "manifest.json").write_text(json.dumps(manifest, indent=2))
    return manifest


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    prepare(args.source, args.output)
