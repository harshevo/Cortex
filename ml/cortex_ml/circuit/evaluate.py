"""Frozen held-out evaluation; answers grade outputs and never become question inputs."""

import os

os.environ["PYTORCH_ENABLE_MPS_FALLBACK"] = "0"

import argparse
import json
from collections import defaultdict
from pathlib import Path

import torch

from .data import dialogue_batch, lessons, token_spikes
from .model import Circuit
from .tokenizer import Tokenizer
from .train import atomic_json, evaluate


@torch.no_grad()
def nll(model, rows, codec, ticks, device):
    loss = 0
    total = 0
    for start in range(0, len(rows), 4):
        x, y = dialogue_batch(rows[start : start + 4], codec, model.config, ticks)
        x = x.to(device)
        y = y.to(device)
        logits, _ = model(x, every_step=True, readout_every=ticks, head="dialogue")
        loss += torch.nn.functional.cross_entropy(
            logits.flatten(0, 1), y.flatten(), ignore_index=-100, reduction="sum"
        ).item()
        total += (y != -100).sum().item()
    return loss / max(total, 1)


@torch.no_grad()
def generate(model, codec, question, ticks, device, max_tokens=40):
    size = len(codec.tokens)
    sequence = [size] + codec.encode(question) + [size + 1]
    x = token_spikes(torch.tensor([sequence]), model.inputs, ticks, size).to(device)
    logits, state = model(x, head="dialogue")
    selected = []
    reason = "length_limit"
    for _ in range(max_tokens):
        token = logits[0].argmax().item()
        if token == 0:
            reason = "end_symbol"
            break
        selected.append(token)
        if len(selected) >= 5 and len(set(selected[-5:])) == 1:
            reason = "repetition_limit"
            break
        x = token_spikes(torch.tensor([[token]]), model.inputs, ticks, size).to(device)
        logits, state = model(x, state=state, head="dialogue")
    return {"text": codec.decode(selected), "tokens": selected, "stop_reason": reason}


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--data", type=Path, required=True)
    p.add_argument("--checkpoint", type=Path, required=True)
    p.add_argument("--output", type=Path, required=True)
    p.add_argument("--device", choices=["cpu", "mps", "cuda"], default="cpu")
    args = p.parse_args()
    torch.set_num_threads(4)
    checkpoint = json.loads(args.checkpoint.read_text())
    codec = Tokenizer(
        tokens=checkpoint["dialogue"]["tokens"], merges=checkpoint["dialogue"]["merges"]
    )
    ticks = checkpoint["dialogue"]["ticks_per_symbol"]
    model = Circuit(checkpoint, dialogue_vocab=len(codec.tokens)).to(args.device)
    operations = lessons(args.data / "operations-test.jsonl")
    english = lessons(args.data / "english-test.jsonl")
    report = evaluate(model, operations, english, codec, ticks, args.device)
    report["answer_domains"] = {}
    for domain in sorted({r["domain"] for r in english}):
        report["answer_domains"][domain] = evaluate(
            model,
            [],
            [r for r in english if r["domain"] == domain],
            codec,
            ticks,
            args.device,
        )
    by_length = defaultdict(list)
    for row in english:
        by_length[len(codec.encode(row["question"]))].append(row)
    original = []
    shuffled = []
    for group in by_length.values():
        if len(group) < 2:
            continue
        for row in group:
            other = next(
                (
                    candidate
                    for candidate in group
                    if candidate["question"] != row["question"]
                ),
                None,
            )
            if other is None:
                continue
            original.append(row)
            shuffled.append({**row, "question": other["question"]})
    report["question_control"] = {
        "controlled_questions": len(original),
        "description": "Replace questions within exact token-length groups, preserving answer prefixes and sequence duration; singleton lengths excluded.",
    }
    if original:
        actual = nll(model, original, codec, ticks, args.device)
        control = nll(model, shuffled, codec, ticks, args.device)
        report["question_control"].update(
            original_nll=actual,
            shuffled_nll=control,
            conditioning_gain=control - actual,
        )
    examples = []
    for row in english[:8]:
        result = generate(model, codec, row["question"], ticks, args.device)
        examples.append(
            {
                "question": row["question"],
                "expected": row["answer"],
                "generated": result,
            }
        )
    report["generation_examples"] = examples
    report["scope"] = (
        "Teacher-forced answer-token metrics are not free-generation accuracy or evidence of broad reasoning. Test answers are never used as prompt text. Question-length matched controls probe input conditioning."
    )
    report["checkpoint"] = str(args.checkpoint)
    atomic_json(args.output, report)
    print(
        json.dumps(
            {
                k: v
                for k, v in report.items()
                if k not in ["generation_examples", "answer_domains"]
            },
            indent=2,
        )
    )


if __name__ == "__main__":
    main()
