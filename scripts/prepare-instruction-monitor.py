#!/usr/bin/env python3
"""Select existing corpus records for a bounded instruction/monitoring experiment."""

import argparse
import gzip
import hashlib
import json
from pathlib import Path


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def prepare(data, checkpoint, output):
    monitor, instructions, seen_questions, seen_groups = [], [], set(), set()
    for split, monitor_count, pair_count in [
        ("train", 102, 192), ("dev", 38, 32), ("test", 54, 32)
    ]:
        candidates = []
        with gzip.open(data / f"{split}.jsonl.gz", "rt") as stream:
            for line in stream:
                record = json.loads(line)
                if record.get("source") != "dolly-15k" or record.get("language") != "en":
                    continue
                turn = record["turns"][0]
                question, answer = turn["question"].strip(), turn["answer"].strip()
                group, key = record["group"], question.casefold()
                if not (
                    8 <= len(question) <= 160
                    and question.isascii()
                    and key not in seen_questions
                    and group not in seen_groups
                    and 0 < len(answer) <= 160
                ):
                    continue
                seen_questions.add(key)
                seen_groups.add(group)
                candidates.append(dict(
                    id=record["id"], group=group, split=split,
                    question=question, answer=answer,
                ))
        candidates.sort(key=lambda row: hashlib.sha256(row["id"].encode()).hexdigest())
        if len(candidates) < max(monitor_count, pair_count):
            raise ValueError(f"Insufficient {split} source examples: {len(candidates)}")
        monitor.extend(
            {key: value for key, value in row.items() if key != "answer"}
            for row in candidates[:monitor_count]
        )
        instructions.extend(candidates[:pair_count])
    manifest = {
        "dataset": str(data),
        "dataset_manifest_sha256": digest(data / "manifest.json"),
        "baseline_sha256": digest(checkpoint),
        "source": "User-provided dolly-15k records only",
        "selection": "English ASCII questions 8..160 characters and source answers <=160 characters; one question per conversation group; deterministic id hash order",
        "counts": {
            split: {
                "monitor_contexts": sum(row["split"] == split for row in monitor),
                "instruction_pairs": sum(row["split"] == split for row in instructions),
            }
            for split in ["train", "dev", "test"]
        },
        "test_exposure_limit": "Held out from this adaptation run, including direction estimation; exposure during earlier checkpoint training is not fully documented",
        "conversation_answers_generated": False,
        "monitor_labels": "Observed intervention versus sham condition, not conversational answers",
    }
    output.mkdir(parents=True, exist_ok=False)
    for filename, value in [
        ("monitor-contexts.json", monitor),
        ("instruction-pairs.json", instructions),
        ("manifest.json", manifest),
    ]:
        (output / filename).write_text(json.dumps(value, indent=2))
    print(json.dumps(manifest, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--data", required=True, type=Path)
    parser.add_argument("--checkpoint", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    prepare(args.data, args.checkpoint, args.output)
