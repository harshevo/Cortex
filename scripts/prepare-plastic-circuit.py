#!/usr/bin/env python3
"""Prepare real local English answers and source-separated circuit experiments."""

import argparse
import gzip
import hashlib
import json
import re
import sys
import xml.etree.ElementTree as ET
from collections import Counter
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "ml"))
from cortex_ml.circuit.tokenizer import Tokenizer, dialogue_tokens


def identity(text):
    return hashlib.sha256(text.strip().lower().encode()).hexdigest()


def records(path):
    with gzip.open(path, "rt") as f:
        for line in f:
            yield json.loads(line)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("root", type=Path)
    p.add_argument("--operations", type=Path, default=Path("target/circuit-research"))
    p.add_argument("--output", type=Path, default=Path("target/circuit-plastic/data"))
    p.add_argument("--train", type=int, default=1000)
    p.add_argument("--evaluation", type=int, default=100)
    args = p.parse_args()
    if not 1 <= args.train <= 16000 or not 1 <= args.evaluation <= 1000:
        p.error("Use 1..16000 training rows and 1..1000 evaluation rows")
    root = args.root.resolve()
    splits = {name: [] for name in ["train", "dev", "test"]}
    source_files = []
    dolly = root / "04_conversation/dolly-15k/records.jsonl.gz"
    source_files.append(dolly)
    for row in records(dolly):
        data = row["data"]
        q = data["instruction"].strip()
        a = data["response"].strip()
        context = data.get("context", "").strip()
        if context:
            q = "Context: " + context + " Question: " + q
        if not q or not a or len(q.encode()) > 350 or len(a.encode()) > 550:
            continue
        key = identity(q)
        bucket = int(key[:8], 16) % 10
        split = "test" if bucket == 0 else "dev" if bucket == 1 else "train"
        splits[split].append(
            {
                "question": q,
                "answer": a,
                "id": key,
                "group": key,
                "source": "dolly-15k",
                "domain": "english_conversation",
            }
        )
    oasst = root / "04_conversation/oasst1/records.jsonl.gz"
    source_files.append(oasst)
    messages = {}
    for row in records(oasst):
        d = row["data"]
        if d.get("lang") == "en" and not d.get("deleted") and not d.get("synthetic"):
            messages[d["message_id"]] = {
                k: d.get(k)
                for k in ["parent_id", "text", "role", "rank", "review_result"]
            }
    for key, d in messages.items():
        parent = messages.get(d["parent_id"])
        if (
            d["role"] != "assistant"
            or d["rank"] not in (None, 0)
            or not parent
            or parent["role"] != "prompter"
        ):
            continue
        q = parent["text"].strip()
        a = d["text"].strip()
        if not q or not a or len(q.encode()) > 350 or len(a.encode()) > 550:
            continue
        root_id = key
        seen = set()
        while messages[root_id]["parent_id"] in messages and root_id not in seen:
            seen.add(root_id)
            root_id = messages[root_id]["parent_id"]
        if messages[root_id]["parent_id"] or root_id in seen:
            continue
        group = identity(root_id)
        bucket = int(group[:8], 16) % 10
        split = "test" if bucket == 0 else "dev" if bucket == 1 else "train"
        splits[split].append(
            {
                "question": q,
                "answer": a,
                "id": identity(q + " " + a),
                "group": group,
                "source": "oasst1",
                "domain": "english_conversation",
            }
        )
    gsm = root / "05_math/gsm8k/records.jsonl.gz"
    source_files.append(gsm)
    for row in records(gsm):
        if not row.get("path", "").endswith(("/train.jsonl", "/test.jsonl")):
            continue
        data = row["data"]
        q = data["question"]
        a = re.sub(r"<<[^<>]*>>", "", data["answer"]).replace("####", "Answer:")
        if len(q.encode()) > 350 or len(a.encode()) > 550:
            continue
        key = identity(q)
        split = (
            "test"
            if row["path"].endswith("/test.jsonl")
            else ("dev" if int(key[:8], 16) % 7 == 0 else "train")
        )
        splits[split].append(
            {
                "question": q,
                "answer": a,
                "id": key,
                "group": key,
                "source": "gsm8k",
                "domain": "math_explanation",
            }
        )
    physics = root / "06_physics/openstax-highschool-physics/documents.jsonl.gz"
    source_files.append(physics)
    for row in records(physics):
        if not row.get("path", "").endswith(".cnxml"):
            continue
        try:
            document = ET.fromstring(row["text"])
        except ET.ParseError:
            continue
        titles = [e for e in document.iter() if e.tag.rsplit("}", 1)[-1] == "title"]
        if not titles:
            continue
        title = " ".join("".join(titles[0].itertext()).split())
        q = "Explain " + title + "."
        group = identity(q)
        bucket = int(group[:8], 16) % 10
        split = "test" if bucket == 0 else "dev" if bucket == 1 else "train"
        for para in document.iter():
            if para.tag.rsplit("}", 1)[-1] != "para":
                continue
            a = " ".join("".join(para.itertext()).split())
            if not 60 <= len(a.encode()) <= 350:
                continue
            key = identity(q + " " + a)
            splits[split].append(
                {
                    "question": q,
                    "answer": a,
                    "id": key,
                    "group": group,
                    "source": "openstax-highschool-physics",
                    "domain": "physics_explanation",
                }
            )
    for name, source_rows in splits.items():
        # Balance sources before applying a bounded experiment size.
        by_source = {
            source: sorted(
                [r for r in source_rows if r["source"] == source],
                key=lambda r: r["id"],
            )
            for source in [
                "dolly-15k",
                "oasst1",
                "gsm8k",
                "openstax-highschool-physics",
            ]
        }
        rows = []
        for i in range(max(map(len, by_source.values()))):
            rows.extend(v[i] for v in by_source.values() if i < len(v))
        limit = args.train if name == "train" else args.evaluation
        splits[name] = rows[: limit * 3]
    # Vocabulary sees training text only. No pretrained tokenizer or embeddings.
    tokenizer = Tokenizer.fit(
        [r["question"] + "\n" + r["answer"] for r in splits["train"]], 512
    )
    for name, rows in splits.items():
        limit = args.train if name == "train" else args.evaluation
        splits[name] = [r for r in rows if len(dialogue_tokens(r, tokenizer)[0]) <= 96][
            :limit
        ]
    train_questions = {identity(r["question"]) for r in splits["train"]}
    for name in ["dev", "test"]:
        splits[name] = [
            r for r in splits[name] if identity(r["question"]) not in train_questions
        ]
    dev_questions = {identity(r["question"]) for r in splits["dev"]}
    splits["test"] = [
        r for r in splits["test"] if identity(r["question"]) not in dev_questions
    ]
    operation_rows = {
        name: [
            json.loads(line)
            for line in (args.operations / f"{name}.jsonl").read_text().splitlines()
        ]
        for name in ["train", "dev", "test"]
    }
    operation_train = {identity(r["question"]) for r in operation_rows["train"]}
    operation_holdout = {
        identity(r["question"])
        for name in ["dev", "test"]
        for r in operation_rows[name]
    }
    audit = args.output / "audit"
    audit.mkdir(parents=True, exist_ok=True)
    (audit / "english-dev-before-cross-stream-filter.jsonl").write_text(
        "".join(json.dumps(row) + "\n" for row in splits["dev"])
    )
    cross_stream_excluded = {}
    for name, source_rows in splits.items():
        forbidden = operation_holdout if name == "train" else operation_train
        before = len(source_rows)
        splits[name] = [
            r for r in source_rows if identity(r["question"]) not in forbidden
        ]
        cross_stream_excluded[name] = before - len(splits[name])
    train_groups = {r["group"] for r in splits["train"]}
    for name in ["dev", "test"]:
        assert not train_groups.intersection(r["group"] for r in splits[name])
    assert not {r["group"] for r in splits["dev"]}.intersection(
        r["group"] for r in splits["test"]
    )
    assert not {r["question"] for r in splits["train"]}.intersection(
        r["question"] for r in splits["dev"] + splits["test"]
    )
    args.output.mkdir(parents=True, exist_ok=True)
    for name, rows in splits.items():
        with (args.output / f"english-{name}.jsonl").open("w") as f:
            for row in rows:
                f.write(json.dumps(row) + "\n")
    (args.output / "tokenizer.json").write_text(json.dumps(tokenizer.json()) + "\n")
    for name in ["train", "dev", "test", "broad"]:
        (args.output / f"operations-{name}.jsonl").write_bytes(
            (args.operations / f"{name}.jsonl").read_bytes()
        )
    manifest = {
        "root": str(root),
        "english": {
            name: dict(Counter(r["domain"] for r in rows))
            for name, rows in splits.items()
        },
        "cross_stream_excluded": cross_stream_excluded,
        "tokenizer": "512 byte-pair tokens fitted on training text only; no pretrained weights",
        "max_symbols": 96,
        "split": "Dolly question hash; GSM8K official split with training hash dev; OpenStax title groups keep whole modules together; OASST root-conversation hash.",
        "sources": {
            str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in source_files
        },
        "scope": "Real answer-token learning, including math/physics explanations. Numeric execution remains a supplied primitive. Existing operation holdouts are reused and reported as such.",
    }
    (args.output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(json.dumps(manifest["english"], indent=2))


if __name__ == "__main__":
    main()
