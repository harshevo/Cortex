"""Memory-bounded 30M circuit training on pretokenized conversation episodes."""

from __future__ import annotations

import hashlib
import json
import math
import os
import random
import tempfile
import time
from pathlib import Path

import numpy as np
import torch
from torch import nn

from .data import sha256, token_spikes
from .portable import Session, Trainer, initialize
from .tokenizer import Tokenizer
from .train import atomic_json


def gpu_budget(gigabytes=5.0):
    """Reserve headroom for CUDA context; allocator limits cannot cover other processes."""
    if not torch.cuda.is_available():
        return {"device": "cpu", "cuda_verified": False}
    total = torch.cuda.get_device_properties(0).total_memory
    budget = min(int(gigabytes * 1_000_000_000), total)
    allocator = max(1, budget - 750_000_000)
    torch.cuda.set_per_process_memory_fraction(allocator / total, 0)
    torch.cuda.reset_peak_memory_stats()
    return {
        "device": torch.cuda.get_device_name(0),
        "budget_bytes": budget,
        "allocator_cap_bytes": allocator,
        "context_headroom_bytes": budget - allocator,
    }


def parameter_report(model):
    groups = {
        name: p.numel() for name, p in model.named_parameters() if p.requires_grad
    }
    total = sum(groups.values())
    return {
        "trainable_parameters": total,
        "groups": groups,
        "fp32_weights_gradients_adam_bytes": total * 16,
        "scope": "Most parameters are token-output synapses; parameter count is not a neuron count or intelligence measure.",
    }


class TokenDataset:
    def __init__(self, directory, split, window=512, verify=True):
        if split not in ("train", "dev", "test") or not 16 <= window <= 4096:
            raise ValueError("Invalid split or window")
        directory = Path(directory)
        self.manifest = json.loads((directory / "manifest.json").read_text())
        if verify:
            for name in (
                f"{split}.tokens.bin",
                f"{split}.targets.bin",
                f"{split}.index.json",
                "tokenizer.json",
            ):
                if sha256(directory / name) != self.manifest["sha256"][name]:
                    raise ValueError(f"Dataset checksum mismatch: {name}")
        self.tokens = np.memmap(
            directory / f"{split}.tokens.bin", dtype="<u2", mode="r"
        )
        self.targets = np.memmap(
            directory / f"{split}.targets.bin", dtype="<i2", mode="r"
        )
        if len(self.tokens) != len(self.targets):
            raise ValueError("Input/target sizes differ")
        self.windows = []
        for row in json.loads((directory / f"{split}.index.json").read_text()):
            end = row["offset"] + row["length"]
            if row["offset"] < 0 or end > len(self.tokens):
                raise ValueError("Invalid episode index")
            for start in range(row["offset"], end, window):
                stop = min(start + window, end)
                if (self.targets[start:stop] != -100).any():
                    self.windows.append((start, stop))
        if not self.windows:
            raise ValueError("No answer-bearing windows")
        self.identity = hashlib.sha256(
            json.dumps(
                [self.manifest["sha256"], split, window], sort_keys=True
            ).encode()
        ).hexdigest()

    def __len__(self):
        return len(self.windows)

    def __getitem__(self, index):
        start, stop = self.windows[index]
        return (
            torch.from_numpy(self.tokens[start:stop].astype(np.int64)),
            torch.from_numpy(self.targets[start:stop].astype(np.int64)),
        )


def atomic_torch(path, payload):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    temp = None
    try:
        with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as stream:
            temp = Path(stream.name)
            torch.save(payload, stream)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temp, path)
    finally:
        if temp is not None:
            temp.unlink(missing_ok=True)


class ConversationTrainer:
    def __init__(
        self,
        model,
        codec,
        *,
        ticks=2,
        chunk=16,
        accumulation=1,
        lr=0.001,
        core_lr=0.0003,
        seed=731,
    ):
        if not 1 <= ticks <= 8 or not 1 <= chunk <= 64 or not 1 <= accumulation <= 16:
            raise ValueError("Use ticks 1..8, chunk 1..64, accumulation 1..16")
        if any(not math.isfinite(v) or not 0 < v <= 0.1 for v in (lr, core_lr)):
            raise ValueError("Invalid learning rate")
        self.model, self.codec = model, codec
        model.output.requires_grad_(
            False
        )  # This corpus trains conversation, not supplied operation labels.
        self.settings = {
            "ticks": ticks,
            "chunk": chunk,
            "accumulation": accumulation,
            "lr": lr,
            "core_lr": core_lr,
            "seed": seed,
        }
        self.optimizer = torch.optim.Adam(
            [
                {"params": model.dialogue_output.parameters(), "lr": lr},
                {
                    "params": [
                        model.magnitudes,
                        model.threshold_scale,
                        model.branch_gain,
                    ],
                    "lr": core_lr,
                },
            ],
            foreach=False,
        )
        self.rng = random.Random(seed)
        self.steps = self.epoch = self.cursor = 0
        self.order = []
        self.data_identity = None
        self.best_score = float("inf")
        self.history = []

    @property
    def device(self):
        return self.model.magnitudes.device

    def window_loss(self, tokens, targets, backward=False):
        state = None
        loss_sum = correct = count = 0
        denominator = max(int((targets != -100).sum()), 1)
        for start in range(0, len(tokens), self.settings["chunk"]):
            chunk = tokens[start : start + self.settings["chunk"]][None]
            gold = targets[start : start + self.settings["chunk"]].to(self.device)
            x = token_spikes(
                chunk, self.model.inputs, self.settings["ticks"], len(self.codec.tokens)
            ).to(self.device)
            enabled = backward and bool((gold != -100).any())
            with torch.set_grad_enabled(enabled):
                logits, state = self.model(
                    x,
                    state=state,
                    every_step=True,
                    readout_every=self.settings["ticks"],
                    head="dialogue",
                )
                valid = gold != -100
                if valid.any():
                    loss = nn.functional.cross_entropy(
                        logits[0], gold, ignore_index=-100, reduction="sum"
                    )
                    if not torch.isfinite(loss):
                        raise RuntimeError("Nonfinite loss")
                    if enabled:
                        (loss / denominator / self.settings["accumulation"]).backward()
                    loss_sum += loss.item()
                    correct += ((logits[0].argmax(-1) == gold) & valid).sum().item()
                    count += valid.sum().item()
            state = state.detach()
            del logits, x
        return loss_sum, correct, count

    def update(self, windows):
        if len(windows) != self.settings["accumulation"]:
            raise ValueError("Window count must equal gradient accumulation")
        self.optimizer.zero_grad(set_to_none=True)
        total = count = 0
        try:
            for tokens, targets in windows:
                loss, _, n = self.window_loss(tokens, targets, backward=True)
                total += loss
                count += n
            nn.utils.clip_grad_norm_(
                self.model.parameters(), 1.0, error_if_nonfinite=True, foreach=False
            )
            self.optimizer.step()
            self.model.constrain()
        except BaseException:
            self.optimizer.zero_grad(set_to_none=True)
            raise
        self.steps += 1
        return total / max(count, 1)

    @torch.no_grad()
    def evaluate(self, data, max_windows=32):
        total = correct = count = 0
        for index in range(min(len(data), max_windows)):
            loss, hits, n = self.window_loss(*data[index])
            total += loss
            correct += hits
            count += n
        return {
            "answer_nll": total / max(count, 1),
            "token_accuracy": correct / max(count, 1),
            "answer_tokens": count,
            "windows": min(len(data), max_windows),
        }

    def next_window(self, data):
        if self.cursor == len(self.order):
            self.order = list(range(len(data)))
            self.rng.shuffle(self.order)
            self.cursor = 0
            self.epoch += 1
        index = self.order[self.cursor]
        self.cursor += 1
        return data[index]

    def save(self, path):
        # No GPU model copy is created; torch serializes tensors to CPU storage.
        atomic_torch(
            path,
            {
                "format": "cortex-conversation-v1",
                "template": self.model.template,
                "codec": self.codec.json(),
                "settings": self.settings,
                "model": self.model.state_dict(),
                "optimizer": self.optimizer.state_dict(),
                "fast_readout": self.model.fast_readout,
                "steps": self.steps,
                "rng": self.rng.getstate(),
                "epoch": self.epoch,
                "cursor": self.cursor,
                "order": self.order,
                "data_identity": self.data_identity,
                "best_score": self.best_score,
                "history": self.history,
            },
        )
        return Path(path)

    @classmethod
    def load(cls, path, device="cpu"):
        payload = torch.load(path, map_location="cpu", weights_only=True)
        if payload.get("format") != "cortex-conversation-v1":
            raise ValueError("Not a conversation training checkpoint")
        codec = Tokenizer(**payload["codec"])
        model = initialize(payload["template"], codec, device=device)
        model.fast_readout = payload["fast_readout"]
        model.load_state_dict(payload["model"])
        result = cls(model, codec, **payload["settings"])
        result.optimizer.load_state_dict(payload["optimizer"])
        result.rng.setstate(payload["rng"])
        for key in (
            "steps",
            "epoch",
            "cursor",
            "order",
            "data_identity",
            "best_score",
            "history",
        ):
            setattr(result, key, payload[key])
        return result

    def fit(
        self,
        train,
        dev,
        output,
        steps=1000,
        checkpoint_every=100,
        dev_windows=32,
        callback=None,
    ):
        if not 1 <= steps or checkpoint_every < 1:
            raise ValueError("Positive steps and checkpoint interval required")
        if self.data_identity not in (None, train.identity):
            raise ValueError("Training data changed since checkpoint")
        self.data_identity = train.identity
        output = Path(output)
        output.mkdir(parents=True, exist_ok=True)
        started = time.perf_counter()

        def checkpoint():
            result = self.evaluate(dev, dev_windows)
            improved = result["answer_nll"] < self.best_score
            if improved:
                self.best_score = result["answer_nll"]
            event = {
                "step": self.steps,
                "epoch": self.epoch,
                "elapsed_seconds": time.perf_counter() - started,
                **result,
                "checkpoint": f"step-{self.steps:08d}.pt",
            }
            if self.device.type == "cuda":
                event.update(
                    gpu_peak_allocated_bytes=torch.cuda.max_memory_allocated(),
                    gpu_peak_reserved_bytes=torch.cuda.max_memory_reserved(),
                )
            self.history.append(event)
            path = self.save(output / event["checkpoint"])
            if improved:
                # Disk copy avoids duplicating the model on the GPU.
                import shutil

                shutil.copy2(path, output / "best.pt")
            atomic_json(output / "progress.json", self.history)
            if callback:
                callback(path, event)
            else:
                print(json.dumps(event), flush=True)

        try:
            if not self.history or not (output / "best.pt").exists():
                if not (output / "best.pt").exists():
                    self.best_score = float("inf")
                checkpoint()
            for i in range(steps):
                cursor, order, epoch, rng = (
                    self.cursor,
                    self.order[:],
                    self.epoch,
                    self.rng.getstate(),
                )
                windows = [
                    self.next_window(train)
                    for _ in range(self.settings["accumulation"])
                ]
                try:
                    self.update(windows)
                except BaseException:
                    self.cursor, self.order, self.epoch = cursor, order, epoch
                    self.rng.setstate(rng)
                    raise
                if (i + 1) % checkpoint_every == 0 or i + 1 == steps:
                    checkpoint()
        finally:
            self.save(output / "latest.pt")
        return self.history[-1]

    def export_rust(self, path):
        # Reuse the compatible JSON serializer without constructing a second GPU model.
        exporter = Trainer(self.model, self.codec, ticks=self.settings["ticks"])
        exporter.steps = self.steps
        return exporter.export(path)

    def session(self, continuous=True):
        return Session(
            self.model, self.codec, self.settings["ticks"], continuous=continuous
        )
