"""Joint recurrent/dendritic BPTT with matched frozen-circuit controls."""

from __future__ import annotations

import os

os.environ["PYTORCH_ENABLE_MPS_FALLBACK"] = "0"

import argparse
import copy
import json
import math
import random
import tempfile
import time
from collections import Counter
from pathlib import Path

import torch
from torch import nn

from .benchmark import benchmark_joint, joint_loss
from .data import dialogue_batch, lessons, operation_batch, sha256
from .model import Circuit
from .tokenizer import Tokenizer


def atomic_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile("w", dir=path.parent, delete=False) as stream:
        temp = Path(stream.name)
        try:
            json.dump(value, stream, allow_nan=False)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        except BaseException:
            temp.unlink(missing_ok=True)
            raise
    os.replace(temp, path)


def operation_tensors(rows, tokenizer, model, ticks, device):
    x, lengths = operation_batch(rows, tokenizer, model.config, ticks)
    target = torch.tensor([model.template["labels"].index(r["label"]) for r in rows])
    return x.to(device), lengths.to(device), target.to(device)


def english_tensors(rows, tokenizer, model, ticks, device):
    return tuple(
        v.to(device) for v in dialogue_batch(rows, tokenizer, model.config, ticks)
    )


@torch.no_grad()
def evaluate(model, operations, english, tokenizer, ticks, device, batch=4):
    op_loss = 0
    correct = 0
    totals = {}
    en_loss = 0
    en_tokens = 0
    token_correct = 0
    for start in range(0, len(operations), batch):
        rows = operations[start : start + batch]
        x, lengths, y = operation_tensors(rows, tokenizer, model, ticks, device)
        logits, _ = model(x, lengths=lengths, readout_every=ticks)
        op_loss += nn.functional.cross_entropy(logits, y, reduction="sum").item()
        hits = (logits.argmax(1) == y).cpu().tolist()
        correct += sum(hits)
        for row, hit in zip(rows, hits):
            count = totals.setdefault(row["domain"], {"correct": 0, "total": 0})
            count["total"] += 1
            count["correct"] += int(hit)
    for start in range(0, len(english), batch):
        x, y = english_tensors(
            english[start : start + batch], tokenizer, model, ticks, device
        )
        logits, _ = model(x, every_step=True, readout_every=ticks, head="dialogue")
        valid = y != -100
        en_loss += nn.functional.cross_entropy(
            logits.flatten(0, 1), y.flatten(), ignore_index=-100, reduction="sum"
        ).item()
        en_tokens += valid.sum().item()
        token_correct += ((logits.argmax(-1) == y) & valid).sum().item()
    return {
        "operation_loss": op_loss / max(len(operations), 1),
        "operation_accuracy": correct / max(len(operations), 1),
        "operation_domains": totals,
        "answer_nll": en_loss / max(en_tokens, 1),
        "answer_perplexity": math.exp(min(en_loss / max(en_tokens, 1), 30)),
        "answer_token_accuracy": token_correct / max(en_tokens, 1),
        "answer_tokens": en_tokens,
    }


def train_candidate(
    base,
    train_op,
    train_en,
    dev_op,
    dev_en,
    tokenizer,
    ticks,
    device,
    steps,
    batch,
    plastic,
    seed,
    rates=(0.01, 0.001, 0.001),
):
    model = copy.deepcopy(base).to(device)
    if not plastic:
        model.frozen_recurrence()
    initial = {name: p.detach().cpu().clone() for name, p in model.named_parameters()}
    head_parameters = list(model.output.parameters())
    language_parameters = list(model.dialogue_output.parameters())
    core_parameters = [
        p
        for p in [model.magnitudes, model.threshold_scale, model.branch_gain]
        if p.requires_grad
    ]
    groups = [
        {"params": head_parameters, "lr": rates[0]},
        {"params": language_parameters, "lr": rates[1]},
    ]
    if core_parameters:
        groups.append({"params": core_parameters, "lr": rates[2]})
    optimizer = torch.optim.Adam(groups)
    rng = random.Random(seed)
    history = []
    baseline = evaluate(model, dev_op, dev_en, tokenizer, ticks, device, batch)
    best = baseline["operation_loss"] + 0.5 * baseline["answer_nll"]
    best_step = 0
    best_state = {k: v.detach().cpu().clone() for k, v in model.state_dict().items()}
    gradient_evidence = {}
    start_time = time.perf_counter()
    for step in range(steps):
        op_rows = [train_op[rng.randrange(len(train_op))] for _ in range(batch)]
        en_rows = [train_en[rng.randrange(len(train_en))] for _ in range(batch)]
        op = operation_tensors(op_rows, tokenizer, model, ticks, device)
        en = english_tensors(en_rows, tokenizer, model, ticks, device)
        optimizer.zero_grad(set_to_none=True)
        loss, (_op_loss, _en_loss) = joint_loss(model, op, en, ticks)
        loss.backward()
        if plastic and step == 0:
            gradient_evidence = {
                name: float(gradient.abs().max().item())
                for name, gradient in {
                    "recurrent_synapses": model.magnitudes.grad[model.recurrent_mask],
                    "dendritic_synapses": model.magnitudes.grad[model.dendritic_mask],
                    "branch_gains": model.branch_gain.grad,
                    "neuronal_thresholds": model.threshold_scale.grad,
                }.items()
            }
            if any(not math.isfinite(v) or v == 0 for v in gradient_evidence.values()):
                raise RuntimeError(
                    f"Missing/nonfinite learning gradient: {gradient_evidence}"
                )
        nn.utils.clip_grad_norm_(model.parameters(), 1, error_if_nonfinite=True)
        optimizer.step()
        model.constrain()
        if step == 0 or (step + 1) % 50 == 0 or step + 1 == steps:
            result = evaluate(model, dev_op, dev_en, tokenizer, ticks, device, batch)
            criterion = result["operation_loss"] + 0.5 * result["answer_nll"]
            if criterion < best:
                best = criterion
                best_state = {
                    k: v.detach().cpu().clone() for k, v in model.state_dict().items()
                }
                best_step = step + 1
            event = {
                "step": step + 1,
                "train_loss": loss.item(),
                "development": result,
                "elapsed_seconds": time.perf_counter() - start_time,
            }
            history.append(event)
            print(
                json.dumps(
                    {
                        "candidate": "plastic" if plastic else "frozen",
                        "step": step + 1,
                        "development_operation_accuracy": result["operation_accuracy"],
                        "development_answer_nll": result["answer_nll"],
                        "elapsed_seconds": event["elapsed_seconds"],
                    }
                ),
                flush=True,
            )
    model.load_state_dict(best_state)
    changes = {
        name: {
            "maximum": (p.detach().cpu() - initial[name]).abs().max().item(),
            "changed": int(
                ((p.detach().cpu() - initial[name]).abs() > 1e-7).sum().item()
            ),
        }
        for name, p in model.named_parameters()
    }
    return model, {
        "plastic": plastic,
        "steps": steps,
        "best_step": best_step,
        "history": history,
        "gradients": gradient_evidence,
        "changes": changes,
        "elapsed_seconds": time.perf_counter() - start_time,
    }


@torch.no_grad()
def export(model, tokenizer, ticks, train_op, path, metadata, device):
    result = model.export(metadata)
    centroids = torch.zeros(len(result["labels"]), model.neurons)
    counts = torch.zeros(len(result["labels"]), dtype=torch.long)
    for start in range(0, len(train_op), 8):
        rows = train_op[start : start + 8]
        x, lengths, y = operation_tensors(rows, tokenizer, model, ticks, device)
        features, _ = model(x, lengths=lengths, readout_every=ticks, head="features")
        centroids.index_add_(0, y.cpu(), features.cpu())
        counts.index_add_(0, y.cpu(), torch.ones_like(y.cpu()))
    result["centroids"] = (centroids / counts.clamp_min(1)[:, None]).tolist()
    result["presentations"] = counts.tolist()
    result["updates"] = metadata["steps"]
    result["readout"]["updates"] = metadata["steps"]
    result["dialogue"] = {
        **tokenizer.json(),
        "ticks_per_symbol": ticks,
        "fast_readout": model.fast_readout,
        "readout": {
            "weights": model.dialogue_output.weight.detach().cpu().tolist(),
            "bias": model.dialogue_output.bias.detach().cpu().tolist(),
            "updates": metadata["steps"],
        },
    }
    # Acceptance is calibrated by the Rust runtime on development data after export.
    result["gates"] = [{"confidence": 0.5, "similarity": -1} for _ in result["labels"]]
    result["threshold"] = 0.5
    atomic_json(path, result)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--initial", type=Path, required=True)
    p.add_argument("--data", type=Path, required=True)
    p.add_argument("--output", type=Path, required=True)
    p.add_argument("--steps", type=int, default=250)
    p.add_argument("--batch", type=int, default=4)
    p.add_argument("--ticks", type=int, default=2)
    p.add_argument("--seed", type=int, default=731)
    p.add_argument("--benchmark-only", action="store_true")
    p.add_argument("--fast-language-trace", action="store_true")
    p.add_argument(
        "--resume",
        action="store_true",
        help="Continue the initial checkpoint weights; optimizer moments restart",
    )
    p.add_argument("--operation-lr", type=float, default=0.01)
    p.add_argument("--language-lr", type=float, default=0.001)
    p.add_argument("--core-lr", type=float, default=0.001)
    args = p.parse_args()
    rates = (args.operation_lr, args.language_lr, args.core_lr)
    if any(not math.isfinite(r) or not 0 < r <= 0.1 for r in rates):
        p.error("Learning rates must be finite and in (0, .1]")
    if (
        not 1 <= args.steps <= 5000
        or not 1 <= args.batch <= 16
        or not 1 <= args.ticks <= 8
    ):
        p.error("Training limits: steps 1..5000, batch 1..16, ticks 1..8")
    args.output.mkdir(parents=True, exist_ok=True)
    torch.manual_seed(args.seed)
    train_op = lessons(args.data / "operations-train.jsonl")
    dev_op = lessons(args.data / "operations-dev.jsonl")
    train_en = lessons(args.data / "english-train.jsonl")
    dev_en = lessons(args.data / "english-dev.jsonl")
    if not all([train_op, dev_op, train_en, dev_en]):
        raise ValueError("Both tasks require nonempty training and development splits")
    tokenizer = Tokenizer(**json.loads((args.data / "tokenizer.json").read_text()))
    initial = json.loads(args.initial.read_text())
    labels = sorted({r["label"] for r in train_op})
    n = len(initial["network"]["neurons"])
    if args.resume and (
        initial["labels"] != labels
        or initial.get("dialogue", {}).get("tokens") != tokenizer.tokens
        or initial.get("dialogue", {}).get("merges") != tokenizer.merges
    ):
        raise ValueError(
            "Resume requires exactly the same learned vocabulary and operation catalog"
        )
    initial["labels"] = labels
    initial["sources"] = dict(
        Counter(r.get("source", "unknown") for r in train_op + train_en)
    )
    if not args.resume:
        initial["readout"] = {
            "weights": [[0.0] * n for _ in initial["labels"]],
            "bias": [0.0] * len(initial["labels"]),
            "updates": 0,
        }
    if not args.resume:
        initial.pop("dialogue", None)
    base = Circuit(initial, dialogue_vocab=len(tokenizer.tokens))
    if args.fast_language_trace:
        base.fast_readout = True
    # Both output heads start from random local initialization; no learned weights are imported.
    if not args.resume:
        nn.init.normal_(base.output.weight, std=0.01)
        nn.init.zeros_(base.output.bias)
    op = operation_tensors(train_op[: args.batch], tokenizer, base, args.ticks, "cpu")
    en = english_tensors(train_en[: args.batch], tokenizer, base, args.ticks, "cpu")
    speed = benchmark_joint(base, op, en, args.ticks, repeats=5, warmup=2, rates=rates)
    atomic_json(args.output / "device-benchmark.json", speed)
    if args.benchmark_only:
        return
    device = speed["selected"]
    torch.set_num_threads(speed["threads"])
    report = {
        "resume_weights": args.resume,
        "optimizer_moments_restarted": True,
        "initial_sha256": sha256(args.initial),
        "operation_head_learning_rate": rates[0],
        "language_head_learning_rate": rates[1],
        "core_learning_rate": rates[2],
        "device": speed,
        "seed": args.seed,
        "data": json.loads((args.data / "manifest.json").read_text()),
        "data_hashes": {p.name: sha256(p) for p in args.data.glob("*.jsonl")},
        "method": "One shared sparse delayed ALIF+dendritic circuit; surrogate BPTT. Fixed topology/signs, learned synaptic magnitudes, neuronal thresholds, branch gains, separate operation/token readouts. No pretrained model.",
    }
    report["initial_development"] = evaluate(
        base.to(device), dev_op, dev_en, tokenizer, args.ticks, device, args.batch
    )
    base = base.to("cpu")
    for plastic in [False, True]:
        name = "plastic" if plastic else "frozen"
        model, result = train_candidate(
            base,
            train_op,
            train_en,
            dev_op,
            dev_en,
            tokenizer,
            args.ticks,
            device,
            args.steps,
            args.batch,
            plastic,
            args.seed,
            rates,
        )
        metadata = {
            "method": "surrogate_bptt" if plastic else "readout_only",
            "recurrent_trained": plastic
            or (
                args.resume
                and initial.get("plasticity", {}).get("method") == "surrogate_bptt"
            )
            or (
                args.resume
                and initial.get("plasticity", {}).get("recurrent_trained", False)
            ),
            "parent_sha256": report["initial_sha256"],
            "device": device,
            "steps": result["best_step"]
            + (initial.get("updates", 0) if args.resume else 0),
            "selected_step": result["best_step"],
            "training_budget": args.steps,
            "seed": args.seed,
            "encoder": "ordered_bpe_question_v1",
            "trained_parameters": [
                "recurrent_synapses",
                "input_synapses",
                "dendritic_synapses",
                "branch_gains",
                "neuronal_thresholds",
                "readouts",
            ]
            if plastic
            else ["readouts"],
        }
        export(
            model,
            tokenizer,
            args.ticks,
            train_op,
            args.output / f"{name}.json",
            metadata,
            device,
        )
        report[name] = result
        atomic_json(args.output / "training-report.json", report)
        del model
        if device == "mps":
            torch.mps.empty_cache()
    print(
        json.dumps(
            {"report": str(args.output / "training-report.json"), "selected": device}
        ),
        flush=True,
    )


if __name__ == "__main__":
    main()
