"""Matched synchronized training-step timings. No CPU fallback is enabled for MPS."""

from __future__ import annotations

import copy
import platform
import statistics
import time

import torch
from torch import nn

from .model import Circuit


def synchronize(device):
    if device == "mps":
        torch.mps.synchronize()
    elif device == "cuda":
        torch.cuda.synchronize()


def percentile(values, fraction):
    return sorted(values)[min(len(values) - 1, int((len(values) - 1) * fraction + 0.5))]


def benchmark(checkpoint, spikes, targets, repeats=5, warmup=2):
    torch.manual_seed(47)
    base = Circuit(checkpoint)
    devices = (
        ["cpu"]
        + (["mps"] if torch.backends.mps.is_available() else [])
        + (["cuda"] if torch.cuda.is_available() else [])
    )
    records = []
    reference = None
    for device in devices:
        start = time.perf_counter()
        try:
            model = copy.deepcopy(base).to(device)
            data = spikes.to(device)
            gold = targets.to(device)
            optimizer = torch.optim.Adam(model.parameters(), lr=0.001)
            synchronize(device)
            setup = time.perf_counter() - start
            model.zero_grad(set_to_none=True)
            prediction, _ = model(data)
            loss = nn.functional.cross_entropy(prediction, gold)
            loss.backward()
            synchronize(device)
            tensors = {
                "logits": prediction.detach().cpu(),
                "synapse_gradient": model.magnitudes.grad.detach().cpu(),
            }
            if reference is None:
                reference = tensors
                parity = {
                    "max_logit_error": 0.0,
                    "max_gradient_error": 0.0,
                    "prediction_agreement": 1.0,
                }
            else:
                parity = {
                    "max_logit_error": (reference["logits"] - tensors["logits"])
                    .abs()
                    .max()
                    .item(),
                    "max_gradient_error": (
                        reference["synapse_gradient"] - tensors["synapse_gradient"]
                    )
                    .abs()
                    .max()
                    .item(),
                    "prediction_agreement": (
                        reference["logits"].argmax(1) == tensors["logits"].argmax(1)
                    )
                    .float()
                    .mean()
                    .item(),
                }
            if not torch.isfinite(tensors["synapse_gradient"]).all():
                raise RuntimeError("Nonfinite synaptic gradients")
            times = []
            for i in range(warmup + repeats):
                synchronize(device)
                start = time.perf_counter()
                optimizer.zero_grad(set_to_none=True)
                prediction, _ = model(data)
                loss = nn.functional.cross_entropy(prediction, gold)
                loss.backward()
                nn.utils.clip_grad_norm_(
                    model.parameters(), 1.0, error_if_nonfinite=True
                )
                optimizer.step()
                model.constrain()
                synchronize(device)
                if i >= warmup:
                    times.append(time.perf_counter() - start)
            records.append(
                {
                    "device": device,
                    "setup_seconds": setup,
                    "p50_seconds": statistics.median(times),
                    "p95_seconds": percentile(times, 0.95),
                    "samples": times,
                    "parity": parity,
                    "parameter_device": str(model.magnitudes.device),
                    "fallback": False,
                }
            )
        except (RuntimeError, NotImplementedError) as error:
            records.append({"device": device, "error": str(error)})
    eligible = [
        r
        for r in records
        if "p50_seconds" in r
        and r["parity"]["prediction_agreement"] == 1
        and r["parity"]["max_logit_error"] < 0.02
        and r["parity"]["max_gradient_error"] < 0.05
    ]
    if not eligible:
        raise RuntimeError("No valid backend completed the actual training workload")
    chosen = min(eligible, key=lambda r: r["p50_seconds"])["device"]
    return {
        "selected": chosen,
        "torch": torch.__version__,
        "platform": platform.platform(),
        "cpu_threads": torch.get_num_threads(),
        "batch": spikes.shape[0],
        "ticks": spikes.shape[1],
        "neurons": base.neurons,
        "synapses": base.magnitudes.numel(),
        "records": records,
        "scope": "Same eager forward/backward/Adam update; warmup excluded; GPU synchronized; transfer/setup separate; MPS fallback disabled.",
    }


def joint_loss(model, operations, english, ticks):
    x, lengths, target = operations
    logits, _ = model(x, lengths=lengths, readout_every=ticks)
    operation = nn.functional.cross_entropy(logits, target)
    x, target = english
    prediction, _ = model(x, every_step=True, readout_every=ticks, head="dialogue")
    language = nn.functional.cross_entropy(
        prediction.flatten(0, 1), target.flatten(), ignore_index=-100
    )
    return operation + 0.5 * language, (operation, language)


def benchmark_joint(
    base, operations, english, ticks, repeats=5, warmup=2, rates=(0.01, 0.001, 0.001)
):
    records = []
    reference = None
    cases = (
        [("cpu", 1), ("cpu", 4), ("cpu", 8)]
        + ([("mps", 4)] if torch.backends.mps.is_available() else [])
        + ([("cuda", 4)] if torch.cuda.is_available() else [])
    )
    for device, threads in cases:
        torch.set_num_threads(threads)
        try:
            start = time.perf_counter()
            model = copy.deepcopy(base).to(device)
            op = tuple(t.to(device) for t in operations)
            en = tuple(t.to(device) for t in english)
            optimizer = torch.optim.Adam(
                [
                    {"params": list(model.output.parameters()), "lr": rates[0]},
                    {
                        "params": list(model.dialogue_output.parameters()),
                        "lr": rates[1],
                    },
                    {
                        "params": [
                            model.magnitudes,
                            model.threshold_scale,
                            model.branch_gain,
                        ],
                        "lr": rates[2],
                    },
                ]
            )
            synchronize(device)
            setup = time.perf_counter() - start
            optimizer.zero_grad(set_to_none=True)
            loss, _ = joint_loss(model, op, en, ticks)
            loss.backward()
            synchronize(device)
            values = {
                "loss": loss.detach().cpu(),
                "gradient": model.magnitudes.grad.detach().cpu(),
            }
            if not torch.isfinite(values["gradient"]).all():
                raise RuntimeError("Nonfinite gradient")
            if reference is None:
                reference = values
            parity = {
                "loss_error": abs(values["loss"].item() - reference["loss"].item()),
                "gradient_max_error": (values["gradient"] - reference["gradient"])
                .abs()
                .max()
                .item(),
                "gradient_close": bool(
                    torch.allclose(
                        values["gradient"], reference["gradient"], atol=3e-4, rtol=0.003
                    )
                ),
            }
            times = []
            for i in range(warmup + repeats):
                synchronize(device)
                start = time.perf_counter()
                optimizer.zero_grad(set_to_none=True)
                loss, _ = joint_loss(model, op, en, ticks)
                loss.backward()
                nn.utils.clip_grad_norm_(model.parameters(), 1, error_if_nonfinite=True)
                optimizer.step()
                model.constrain()
                synchronize(device)
                if i >= warmup:
                    times.append(time.perf_counter() - start)
            records.append(
                {
                    "device": device,
                    "threads": threads,
                    "p50_seconds": statistics.median(times),
                    "p95_seconds": percentile(times, 0.95),
                    "samples": times,
                    "setup_seconds": setup,
                    "parity": parity,
                    "parameter_device": str(model.magnitudes.device),
                    "fallback": False,
                }
            )
            print(
                f"benchmark {device}/{threads}: {statistics.median(times):.4f}s per complete joint update",
                flush=True,
            )
            del model, optimizer
            if device == "mps":
                torch.mps.empty_cache()
        except (RuntimeError, NotImplementedError) as error:
            records.append({"device": device, "threads": threads, "error": str(error)})
    valid = [
        r
        for r in records
        if "p50_seconds" in r
        and r["parity"]["gradient_close"]
        and r["parity"]["loss_error"] < 0.02
    ]
    if not valid:
        raise RuntimeError("All backends failed the joint training/parity gate")
    winner = min(valid, key=lambda r: r["p50_seconds"])
    return {
        "selected": winner["device"],
        "threads": winner["threads"],
        "records": records,
        "torch": torch.__version__,
        "platform": platform.platform(),
        "operation_shape": list(operations[0].shape),
        "english_shape": list(english[0].shape),
        "neurons": base.neurons,
        "synapses": base.magnitudes.numel(),
        "scope": "Complete shared-core operation+answer-token forward/backward/Adam updates; synchronized GPU, no MPS fallback; setup/transfer measured separately.",
    }
