#!/usr/bin/env python3
"""Use original corpus answers and hold out fresh conversation groups."""
import argparse
import gzip
import hashlib
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "ml"))
from cortex_ml.circuit.tokenizer import Tokenizer


def prepare(args):
    checkpoint = json.loads(args.checkpoint.read_text())
    codec = Tokenizer(checkpoint["dialogue"]["tokens"], checkpoint["dialogue"]["merges"])
    backend = codec.fast_backend()
    excluded = set()
    for path in args.exclude:
        excluded.update(row["group"] for row in json.loads(path.read_text()) if row["split"] == "test")
    examples, contexts, groups, questions = [], [], {}, {}
    duplicate_questions=0
    for split, limit, monitor_count in [("train", args.train_limit, 102), ("dev", args.dev_limit, 38), ("test", args.test_limit, 54)]:
        candidates, seen = [], set()
        with gzip.open(args.data / f"{split}.jsonl.gz", "rt") as stream:
            for line in stream:
                row = json.loads(line)
                if (args.language!='all' and row.get("language") != args.language) or row["group"] in excluded:
                    continue
                turn = row["turns"][0]
                question, answer = turn["question"].strip(), turn["answer"].strip()
                canonical = " ".join(question.casefold().split())
                if row['group'] in groups and groups[row['group']]!=split:raise ValueError('Corpus group crosses splits')
                groups[row['group']]=split
                if canonical in questions and questions[canonical]!=split:
                    duplicate_questions+=1;continue
                questions[canonical]=split
                if row["group"] in seen or not question or not answer:
                    continue
                tokens = backend.encode(question).ids
                target = backend.encode(answer).ids + [0]
                if not 4 <= len(tokens) <= args.max_input or not 2 <= len(target) <= args.max_target:
                    continue
                seen.add(row["group"])
                candidates.append(dict(id=row["id"],group=row["group"],split=split,
                    source=row["source"],question=question,answer=answer,input=tokens,target=target))
        candidates.sort(key=lambda row: hashlib.sha256(row["id"].encode()).hexdigest())
        if limit==0:limit=len(candidates)
        if len(candidates) < max(limit, monitor_count):
            limit = min(limit, len(candidates))
        if len(candidates) < monitor_count:
            raise ValueError(f"Not enough {split} examples")
        examples.extend(candidates[:limit])
        contexts.extend({key: row[key] for key in ["id","group","split","question"]}
                        for row in candidates[-monitor_count:])
    args.output.mkdir(parents=True, exist_ok=False)
    for name, rows in [("pairs.json", examples),("contexts.json", contexts)]:
        (args.output/name).write_text(json.dumps(rows))
    manifest = dict(source=str(args.data), source_manifest_sha256=hashlib.sha256(
        (args.data/'manifest.json').read_bytes()).hexdigest(),
        counts={split:sum(row['split']==split for row in examples) for split in ['train','dev','test']},
        exclusions=len(excluded), duplicate_questions_quarantined=duplicate_questions,generated_conversation_answers=False,
        limits=f"First turns, language={args.language}, 4..{args.max_input} input symbols, 2..{args.max_target} target symbols. Groups and canonical questions isolated; old monitoring/instruction test groups excluded. Historical checkpoint exposure remains unknown.")
    (args.output/'manifest.json').write_text(json.dumps(manifest,indent=2))
    print(json.dumps(manifest,indent=2))


if __name__ == '__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    for name in ['data','checkpoint','output']:parser.add_argument('--'+name,type=Path,required=True)
    parser.add_argument('--exclude',type=Path,action='append',default=[])
    parser.add_argument('--train-limit',type=int,default=6000);parser.add_argument('--dev-limit',type=int,default=192);parser.add_argument('--test-limit',type=int,default=192)
    parser.add_argument('--max-input',type=int,default=128);parser.add_argument('--max-target',type=int,default=64)
    parser.add_argument('--language',default='en')
    prepare(parser.parse_args())
