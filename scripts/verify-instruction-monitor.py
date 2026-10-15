#!/usr/bin/env python3
"""Check exact instructions and Python/Rust detector parity on held-out records."""

import argparse
import hashlib
import json
import subprocess
from collections import Counter
from pathlib import Path


def verify(args):
    pairs = [
        row for row in json.loads(args.pairs.read_text()) if row["split"] == "test"
    ]
    if len(pairs) < 10:
        raise ValueError("Need ten held-out instruction records")
    tests = []
    for index, row in enumerate(pairs[:10]):
        value = " ".join(row["question"].split()[:5])
        a, b = 17 + index * 13, 9 + index * 7
        tests.extend([
            ("copy", "repeat exactly: " + value, value),
            ("case", "uppercase: " + value, value.upper()),
            ("case", "lowercase: " + value, value.lower()),
            ("reverse", "reverse: " + value, value[::-1]),
            ("arithmetic", f"calculate: ({a} + {b}) * 2", str((a + b) * 2)),
            ("json", f"Reply in JSON: calculate: {a} + {b}",
             json.dumps({"answer": str(a + b)}, separators=(",", ":"))),
            ("natural quoted copy", f'Please say exactly "{value}"', value),
            ("natural case", f'Convert "{value}" to uppercase', value.upper()),
            ("boolean", f"Is {a} + {b} equal to {a + b}? Reply B for yes or A for no.", "B"),
            ("boolean", f"Is {a} + {b} equal to {a + b + 1}? Reply A for yes or B for no.", "B"),
        ])
    results = []
    for kind, prompt, expected in tests:
        response = json.loads(subprocess.check_output([
            str(args.binary.resolve()), "generate", str(args.checkpoint), prompt,
        ]))
        results.append(dict(
            kind=kind, prompt=prompt, expected=expected, actual=response["text"],
            method=response["method"], correct=response["text"] == expected,
        ))
    counts = Counter(row["kind"] for row in results)
    correct = Counter(row["kind"] for row in results if row["correct"])
    states = json.loads(args.states.read_text())
    predictions = json.loads(args.predictions.read_text())

    def key(row):
        return row["id"], row["direction"], round(row["strength"], 5), row["injected"]

    expected = {key(row): row for row in predictions}
    rows = [row for row in states["rows"] if row["split"] == "test"]
    if not rows or len(rows) != len(predictions):
        raise ValueError("Held-out prediction/state count mismatch")
    deltas, format_ok, reports_correct = [], 0, 0
    for row in rows:
        monitor = row["language_decision"]["trained_self_monitor"]
        deltas.append(abs(
            monitor["anomaly_probability"] - expected[key(row)]["anomaly_probability"]
        ))
        assert monitor["eligible_state"] and monitor["calibration_matches_core"]
        report = row["instruction_report"]
        assert report and not report.get("abstained", False)
        text = report["text"]
        yes, no = ("A", "B") if row["yes_is_A"] else ("B", "A")
        format_ok += text in ["A", "B"]
        reports_correct += text == (yes if row["injected"] else no)
    proof = {
        "instruction_cases": len(results),
        "correct": sum(row["correct"] for row in results),
        "categories": {key: {"cases": value, "correct": correct[key]} for key, value in counts.items()},
        "instruction_scope": "Exact supplied text/arithmetic/format execution with unseen values derived from held-out source questions; not learned free-form understanding",
        "monitor_test_trials": len(rows),
        "monitor_groups": len({row["group"] for row in rows}),
        "max_python_rust_probability_error": max(deltas),
        "reported_A_B_format_compliance": format_ok / len(rows),
        "reported_A_B_accuracy_all_conditions": reports_correct / len(rows),
        "language_head_natural_A_B_compliance_after_adaptation": sum(
            row["language_decision"]["first_token_obeys_A_B_instruction"] for row in rows
        ) / len(rows),
        "checkpoint_sha256": hashlib.sha256(args.checkpoint.read_bytes()).hexdigest(),
    }
    assert max(deltas) < 0.00001
    assert proof["correct"] == len(results)
    args.output.mkdir(parents=True, exist_ok=True)
    for filename, value in [("verification.json", proof), ("instruction-cases.json", results)]:
        (args.output / filename).write_text(json.dumps(value, indent=2))
    print(json.dumps(proof, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ["binary", "checkpoint", "pairs", "states", "predictions", "output"]:
        parser.add_argument("--" + name, required=True, type=Path)
    verify(parser.parse_args())
