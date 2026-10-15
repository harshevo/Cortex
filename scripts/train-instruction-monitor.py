#!/usr/bin/env python3
"""Adapt the existing token head and train a separate neural-state anomaly readout."""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
from pathlib import Path

import numpy as np
import torch
from torch import nn


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def atomic_json(path, value):
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, separators=(",", ":")))
    temporary.replace(path)


def assert_splits(rows):
    seen = {}
    for row in rows:
        key = row["group"]
        if key in seen and seen[key] != row["split"]:
            raise ValueError("Conversation group crosses experiment splits")
        seen[key] = row["split"]


def fit_detector(features, labels, split, columns=None):
    train = split == "train"
    dev = split == "dev"
    selected = features if columns is None else features[:, columns]
    mean = selected[train].mean(0)
    scale = selected[train].std(0)
    scale = torch.where(scale > 1e-6, scale, torch.ones_like(scale))
    normalized = ((selected - mean) / scale).clamp(-8, 8)
    head = nn.Linear(selected.shape[1], 2)
    nn.init.zeros_(head.weight)
    nn.init.zeros_(head.bias)
    optimizer = torch.optim.AdamW(head.parameters(), lr=0.02, weight_decay=0.02)
    best, best_loss = copy.deepcopy(head.state_dict()), float("inf")
    for step in range(600):
        optimizer.zero_grad(set_to_none=True)
        loss = nn.functional.cross_entropy(head(normalized[train]), labels[train])
        loss.backward()
        optimizer.step()
        if (step + 1) % 25 == 0:
            with torch.no_grad():
                score = nn.functional.cross_entropy(
                    head(normalized[dev]), labels[dev]
                ).item()
            if score < best_loss:
                best, best_loss = copy.deepcopy(head.state_dict()), score
    head.load_state_dict(best)
    with torch.no_grad():
        probabilities = head(normalized).softmax(-1)[:, 1].numpy()
    return head, mean, scale, probabilities, best_loss


def metrics(rows, probabilities, seed=731):
    labels = np.array([row["injected"] for row in rows], dtype=bool)
    predictions = probabilities >= 0.5
    hit = float(predictions[labels].mean())
    false_alarm = float(predictions[~labels].mean())
    groups = {}
    for row, correct in zip(rows, predictions == labels):
        groups.setdefault(row["group"], []).append(float(correct))
    group_scores = np.array([np.mean(values) for values in groups.values()])
    generator = np.random.default_rng(seed)
    bootstrap = generator.choice(
        group_scores, (2000, len(group_scores)), replace=True
    ).mean(1)
    return {
        "trials": len(rows),
        "question_groups": len(groups),
        "balanced_accuracy": (hit + 1 - false_alarm) / 2,
        "hit_rate": hit,
        "false_alarm_rate": false_alarm,
        "brier_score": float(np.square(probabilities - labels).mean()),
        "group_bootstrap_accuracy_interval_95": np.quantile(
            bootstrap, [0.025, 0.975]
        ).tolist(),
        "interval_scope": "Descriptive resampling of held-out question groups, not a consciousness probability",
    }


def adapt_language(checkpoint, directory):
    index = json.loads((directory / "index.json").read_text())
    examples = [row["record"] for row in index["examples"]]
    assert_splits(examples)
    shape = (index["rows"], index["features"])
    features = torch.from_numpy(
        np.memmap(directory / "features.f32", dtype="<f4", mode="c", shape=shape)
    )
    targets = torch.from_numpy(
        np.memmap(directory / "targets.u16", dtype="<u2", mode="r").astype(np.int64)
    )
    positions = {split: [] for split in ("train", "dev", "test")}
    for row in index["examples"]:
        positions[row["record"]["split"]].extend(
            range(row["offset"], row["offset"] + row["length"])
        )
    positions = {split: torch.tensor(values) for split, values in positions.items()}
    head = nn.Linear(shape[1], index["vocabulary"])
    with torch.no_grad():
        head.weight.copy_(torch.tensor(checkpoint["dialogue"]["readout"]["weights"]))
        head.bias.copy_(torch.tensor(checkpoint["dialogue"]["readout"]["bias"]))

    @torch.no_grad()
    def evaluate(indices):
        total = correct = 0
        for batch in indices.split(256):
            logits = head(features[batch])
            total += nn.functional.cross_entropy(
                logits, targets[batch], reduction="sum"
            ).item()
            correct += (logits.argmax(-1) == targets[batch]).sum().item()
        return {
            "answer_nll": total / len(indices),
            "token_accuracy": correct / len(indices),
            "tokens": len(indices),
        }

    before_dev = evaluate(positions["dev"])
    before_test = evaluate(positions["test"])
    best = copy.deepcopy(head.state_dict())
    best_loss = before_dev["answer_nll"]
    best_step = 0
    optimizer = torch.optim.AdamW(head.parameters(), lr=0.003, weight_decay=0.01)
    history = []
    for step in range(240):
        sample = positions["train"][torch.randint(len(positions["train"]), (256,))]
        optimizer.zero_grad(set_to_none=True)
        loss = nn.functional.cross_entropy(head(features[sample]), targets[sample])
        loss.backward()
        nn.utils.clip_grad_norm_(head.parameters(), 1.0)
        optimizer.step()
        if (step + 1) % 20 == 0:
            score = evaluate(positions["dev"])
            history.append({"step": step + 1, **score})
            if score["answer_nll"] < best_loss:
                best, best_loss, best_step = (
                    copy.deepcopy(head.state_dict()),
                    score["answer_nll"],
                    step + 1,
                )
            print(json.dumps({"language_step": step + 1, **score}), flush=True)
    head.load_state_dict(best)
    after_test = evaluate(positions["test"])
    checkpoint["dialogue"]["readout"]["weights"] = head.weight.detach().tolist()
    checkpoint["dialogue"]["readout"]["bias"] = head.bias.detach().tolist()
    checkpoint["dialogue"]["readout"]["updates"] += best_step
    checkpoint["plasticity"] = {
        **checkpoint.get("plasticity", {}),
        "instruction_adaptation": {
            "method": "frozen_core_source_answer_readout",
            "steps": best_step,
            "train_examples": len([x for x in examples if x["split"] == "train"]),
        },
    }
    return {
        "before_dev": before_dev,
        "before_test": before_test,
        "after_test": after_test,
        "selected_step": best_step,
        "history": history,
        "scope": "Teacher-forced answer-token quality on existing corpus pairs; not a general understanding benchmark",
    }


def train(args):
    torch.manual_seed(731)
    torch.set_num_threads(4)
    checkpoint = json.loads(args.checkpoint.read_text())
    data = json.loads(args.states.read_text())
    rows = data["rows"]
    assert_splits(rows)
    features = torch.tensor([row["features"] for row in rows], dtype=torch.float32)
    labels = torch.tensor([int(row["injected"]) for row in rows])
    split = np.array([row["split"] for row in rows])
    if features.shape[1] != 84 or not torch.isfinite(features).all():
        raise ValueError("Invalid neural activity features")
    # No trial metadata or intervention labels enter either detector's feature vector.
    head, mean, scale, prediction, dev_loss = fit_detector(features, labels, split)
    rate = features[:, [2 + region * 14 for region in range(6)]].mean(1, keepdim=True)
    _, _, _, rate_prediction, _ = fit_detector(rate, labels, split)
    checkpoint["self_monitor"] = {
        "readout": {
            "weights": head.weight.detach().tolist(),
            "bias": head.bias.detach().tolist(),
            "updates": 600,
        },
        "mean": mean.tolist(),
        "scale": scale.tolist(),
        "trained_core_signature": data["core_signature"],
        "training": {
            "method": "supervised_activity_anomaly_detector",
            "feature_count": 84,
            "selection": "development loss only",
            "training_groups": len({r["group"] for r in rows if r["split"] == "train"}),
            "labels": "instrumented intervention/sham conditions",
        },
    }
    conditions = []
    for kind in ("observed_activity", "matched_random"):
        for strength in (0.1, 0.3, 1.0, 0.2, 0.6):
            indices = [
                i
                for i, row in enumerate(rows)
                if row["split"] == "test"
                and row["direction"] == kind
                and abs(row["strength"] - strength) < 1e-6
            ]
            subset = [rows[i] for i in indices]
            conditions.append(
                {
                    "direction": kind,
                    "strength": strength,
                    "trained_monitor": metrics(subset, prediction[indices]),
                    "rate_only_baseline": metrics(subset, rate_prediction[indices]),
                    "language_A_B_compliance_before_adaptation": sum(
                        row["language_decision"]["first_token_obeys_A_B_instruction"]
                        for row in subset
                    )
                    / len(subset),
                }
            )
    recent = features[:, [2 + region * 14 for region in range(6)]].mean(1)
    normal_training = (split == "train") & (~labels.numpy().astype(bool))
    checkpoint["self_monitor"]["training"]["minimum_recent_activity"] = (
        float(recent[normal_training].min()) * 0.25
    )
    checkpoint["self_monitor"]["training"]["calibrated_state_domain"] = (
        "Post-question activity; inactive and other lifecycle states were not validated"
    )
    language = adapt_language(checkpoint, args.cache)
    args.output.mkdir(parents=True, exist_ok=True)
    report = {
        "seed": 731,
        "primary_condition": {"direction": "observed_activity", "strength": 0.3},
        "conditions": conditions,
        "monitor_dev_log_loss": dev_loss,
        "language_adaptation": language,
        "source_checkpoint_sha256": digest(args.checkpoint),
        "state_dataset_sha256": digest(args.states),
        "test_scope": "Questions, conversation groups, prompt templates and activity-direction sources withheld from this adaptation; earlier checkpoint exposure is not fully documented",
        "limits": [
            "This is a newly trained auxiliary activity-anomaly detector, not evidence that the original language head introspects.",
            "Formatting compliance supplied by an instruction layer is not learned natural-language competence.",
            "Activity statistics can detect low-level disturbance; compare the rate-only baseline.",
            "No result establishes subjective consciousness.",
        ],
    }
    atomic_json(args.output / "checkpoint.json", checkpoint)
    atomic_json(args.output / "training-results.json", report)
    predictions = [
        {
            "id": row["id"],
            "group": row["group"],
            "split": row["split"],
            "direction": row["direction"],
            "strength": row["strength"],
            "injected": row["injected"],
            "anomaly_probability": float(prediction[i]),
            "rate_baseline_probability": float(rate_prediction[i]),
        }
        for i, row in enumerate(rows)
        if row["split"] == "test"
    ]
    atomic_json(args.output / "heldout-predictions.json", predictions)
    print(
        json.dumps(
            {
                "saved": str(args.output / "checkpoint.json"),
                "primary": conditions[1],
                "language": {k: v for k, v in language.items() if k != "history"},
            }
        ),
        flush=True,
    )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--checkpoint", required=True, type=Path)
    parser.add_argument("--states", required=True, type=Path)
    parser.add_argument("--cache", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    train(parser.parse_args())
