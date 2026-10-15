#!/usr/bin/env python3
"""Prepare bounded, deterministic brain experiments from the local collection.

No downloads. Test answers are used only as grading references. Equations
extracted from worked solutions form a separate arithmetic-execution test,
not a word-problem-solving score.
"""

import argparse
import gzip
import hashlib
import json
import pathlib
import re


def records(root, source):
    path = root / source / "records.jsonl.gz"
    with gzip.open(path, "rt", encoding="utf-8") as stream:
        for line in stream:
            yield json.loads(line)


def fingerprint(question):
    return hashlib.sha256(question.strip().lower().encode()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=pathlib.Path, help="datasets directory")
    parser.add_argument("--output", type=pathlib.Path, default=pathlib.Path("target/brain-research"))
    parser.add_argument("--train", type=int, default=32, help="examples per training source")
    parser.add_argument("--test", type=int, default=32, help="examples per evaluation source")
    args = parser.parse_args()
    if not (1 <= args.train <= 256 and 1 <= args.test <= 256):
        parser.error("Use 1 to 256 examples per source.")
    train, test, equations = [], [], []
    for record in records(args.root, "05_math/gsm8k"):
        data = record["data"]
        path = record.get("path", "")
        if not path.endswith(("/train.jsonl", "/test.jsonl")):
            continue
        question, answer = data["question"], data["answer"]
        item = dict(domain="math_word_problem", question=question, expected=answer,
                    source="gsm8k", source_path=path, id=fingerprint(question),
                    reference=answer.split("####")[-1].strip())
        if path.endswith("/train.jsonl"):
            if len(question) + len(answer) <= 4000:
                train.append(item)
        else:
            test.append(item)
            for expression, answer in re.findall(r"<<([^=<>]+)=([^<>]+)>>", answer):
                if not re.fullmatch(r"[\d\s.+\-*/()]+", expression):
                    continue
                equations.append(dict(domain="solution_equation_execution", question=expression,
                                      expected=answer, reference=answer, source="gsm8k_test_solution",
                                      source_path=path, id=fingerprint(expression),
                                      note="Supplied expression from a worked solution; NOT autonomous word-problem reasoning."))
    train = sorted(train, key=lambda r: r["id"])[:args.train]
    test = sorted(test, key=lambda r: r["id"])[:args.test]
    equations = list({row["id"]: row for row in equations}.values())
    test += sorted(equations, key=lambda r: r["id"])[:args.test]

    conversation_train, conversation_test = [], []
    for record in records(args.root, "04_conversation/dolly-15k"):
        data = record["data"]
        question = data["instruction"]
        context = data.get("context", "").strip()
        if context:
            question = f"Context: {context}\nQuestion: {question}"
        answer = data["response"]
        if len(question) + len(answer) > 4000:
            continue
        identifier = fingerprint(question)
        row = dict(domain="conversation", question=question, expected=answer,
                   source="dolly-15k", source_path=record["path"], id=identifier)
        # Source has no official held-out split. Hash split is fixed before sampling.
        (conversation_test if int(identifier[:8], 16) % 5 == 0 else conversation_train).append(row)
    train += sorted(conversation_train, key=lambda r: r["id"])[:args.train]
    test += sorted(conversation_test, key=lambda r: r["id"])[:args.test]

    physics = []
    for record in records(args.root, "06_physics/scibench"):
        data = record["data"]
        question = data.get("problem_text", "")
        if not question or len(question) > 3500:
            continue
        physics.append(dict(domain="science_word_problem", question=question,
                            expected=str(data.get("answer_number", data.get("answer_latex", ""))),
                            source="scibench_evaluation_only", source_path=record["path"], id=fingerprint(question)))
    test += sorted(physics, key=lambda r: r["id"])[:args.test]
    training_ids = {row["id"] for row in train}
    assert not training_ids.intersection(row["id"] for row in test), "Train/test question leakage"
    args.output.mkdir(parents=True, exist_ok=True)
    for name, rows in [("train.jsonl", train), ("test.jsonl", test)]:
        with (args.output / name).open("w", encoding="utf-8") as stream:
            for row in rows:
                stream.write(json.dumps(row, ensure_ascii=False) + "\n")
    manifest = dict(root=str(args.root.resolve()), training=len(train), evaluation=len(test),
                    seed="sha256 question identity; conversation bucket modulo 5",
                    split_policy="GSM8K official train/test, Dolly deterministic hash split, SciBench evaluation only",
                    training_sha256=hashlib.sha256((args.output / "train.jsonl").read_bytes()).hexdigest(),
                    evaluation_sha256=hashlib.sha256((args.output / "test.jsonl").read_bytes()).hexdigest(),
                    limitations=["Bounded samples, not whole-dataset scores", "Exact-answer grading of open conversation is strict and incomplete",
                                 "Solution-equation execution is separate from solving word problems"])
    (args.output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(json.dumps(manifest, indent=2))


if __name__ == "__main__":
    main()
