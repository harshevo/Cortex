"""Portable notebook training, resumable state and experimental continuous sessions."""

from __future__ import annotations

import copy
import hashlib
import json
import math
import os
import random
import tempfile
import time
from collections import deque
from pathlib import Path

import torch
from torch import nn

from .data import operation_batch, sha256, token_spikes
from .model import Circuit
from .senses import sensory_spikes
from .tokenizer import Tokenizer, dialogue_tokens
from .train import atomic_json


def select_device(preference="auto"):
    available = {
        "cpu": True,
        "cuda": torch.cuda.is_available(),
        "mps": torch.backends.mps.is_available(),
    }
    if preference == "auto":
        return next(d for d in ("cuda", "mps", "cpu") if available[d])
    if preference not in available or not available[preference]:
        raise ValueError(f"Requested device {preference!r} is unavailable: {available}")
    return preference


def load_dataset(directory):
    directory = Path(directory)
    manifest = json.loads((directory / "manifest.json").read_text())
    for name, expected in manifest["sha256"].items():
        if Path(name).name != name or sha256(directory / name) != expected:
            raise ValueError(f"Dataset checksum mismatch: {name}")
    result = {}
    groups, questions = {}, {}
    for split in ("train", "dev", "test"):
        rows = []
        with (directory / f"{split}.jsonl").open(encoding="utf-8") as stream:
            for line in stream:
                if not line.strip():
                    continue
                row = json.loads(line)
                turns = row.get("turns", [row])
                if not turns or len(turns) > 16:
                    raise ValueError("An episode requires 1..16 turns")
                for turn in turns:
                    for key in ("question", "answer"):
                        if (
                            not isinstance(turn.get(key), str)
                            or not turn[key].strip()
                            or len(turn[key].encode()) > 8192
                        ):
                            raise ValueError(f"Invalid {key} in {split}")
                group = row["group"]
                prompt = json.dumps(
                    {
                        "questions": [t["question"].strip().casefold() for t in turns],
                        "observation": row.get("observation"),
                    },
                    sort_keys=True,
                )
                for seen, key in ((groups, group), (questions, prompt)):
                    if key in seen and seen[key] != split:
                        raise ValueError(f"Train/dev/test leakage for {key}")
                    seen[key] = split
                rows.append(row)
                if len(rows) > 100_000:
                    raise ValueError("Notebook limit: 100000 rows per split")
        if not rows:
            raise ValueError(f"Empty {split} split")
        result[split] = rows
    return result, manifest


def language_batch(rows, codec, config, ticks, max_symbols=192):
    examples = []
    for row in rows:
        sequence, targets = [], []
        for turn in row.get("turns", [row]):
            seq, gold = dialogue_tokens(turn, codec)
            # Feed the terminator between turns, matching continuous inference.
            sequence.extend(seq + [0])
            targets.extend(gold + [-100])
        if len(sequence) > max_symbols:
            raise ValueError(
                f"Example needs {len(sequence)} symbols; limit is {max_symbols}. Shorten it or raise max_symbols."
            )
        x = token_spikes(
            torch.tensor([sequence]), config["inputs"], ticks, len(codec.tokens)
        )[0]
        y = torch.tensor(targets)
        if "observation" in row:
            prefix = sensory_spikes(
                row["observation"], config["inputs"], ticks=16 * ticks
            )[0]
            x = torch.cat([prefix, x])
            y = torch.cat([torch.full((16,), -100), y])
        examples.append((x, y))
    length = max(len(y) for _, y in examples)
    x = torch.zeros(len(rows), length * ticks, config["inputs"])
    y = torch.full((len(rows), length), -100, dtype=torch.long)
    for i, (spikes, target) in enumerate(examples):
        x[i, : len(spikes)] = spikes
        y[i, : len(target)] = target
    return x, y


def initialize(checkpoint, codec, seed=731, device="auto"):
    torch.manual_seed(seed)
    checkpoint = copy.deepcopy(checkpoint)
    dialogue = checkpoint.get("dialogue")
    if dialogue and (
        dialogue["tokens"] != codec.tokens or dialogue["merges"] != codec.merges
    ):
        raise ValueError("Existing weights require their original tokenizer")
    model = Circuit(checkpoint, dialogue_vocab=len(codec.tokens)).to(
        select_device(device)
    )
    if not dialogue:
        nn.init.normal_(model.output.weight, std=0.01)
        nn.init.zeros_(model.output.bias)
    return model


class Trainer:
    def __init__(
        self,
        model,
        codec,
        *,
        ticks=2,
        batch_size=4,
        seed=731,
        head_lr=0.001,
        core_lr=0.0003,
        max_symbols=192,
    ):
        if model.template.get('sequence_decoder'):
            raise ValueError('Use train-learned-instructions.py to train the learned sequence decoder')
        if (
            not 1 <= ticks <= 8
            or not 1 <= batch_size <= 32
            or not 8 <= max_symbols <= 512
        ):
            raise ValueError("Use ticks 1..8, batch_size 1..32, max_symbols 8..512")
        if any(not math.isfinite(v) or not 0 < v <= 0.1 for v in (head_lr, core_lr)):
            raise ValueError("Learning rates must be finite in (0, 0.1]")
        self.model, self.codec = model, codec
        self.settings = {
            "ticks": ticks,
            "batch_size": batch_size,
            "seed": seed,
            "head_lr": head_lr,
            "core_lr": core_lr,
            "max_symbols": max_symbols,
        }
        self.optimizer = torch.optim.Adam(
            [
                {
                    "params": list(model.output.parameters())
                    + list(model.dialogue_output.parameters()),
                    "lr": head_lr,
                },
                {
                    "params": [
                        model.magnitudes,
                        model.branch_gain,
                        model.threshold_scale,
                    ],
                    "lr": core_lr,
                },
            ]
        )
        self.rng = random.Random(seed)
        self.steps = self.replay_steps = 0
        self.history = []
        self.replay = deque(maxlen=256)
        self.data_fingerprint = None
        self.best_score = float("inf")
        self.best_step = 0
        self.best_checkpoint = None

    @property
    def device(self):
        return self.model.magnitudes.device

    def tensors(self, rows):
        return tuple(
            t.to(self.device)
            for t in language_batch(
                rows,
                self.codec,
                self.model.config,
                self.settings["ticks"],
                self.settings["max_symbols"],
            )
        )

    def update(self, rows):
        self.model.train()
        x, y = self.tensors(rows)
        self.optimizer.zero_grad(set_to_none=True)
        logits, _ = self.model(
            x, every_step=True, readout_every=self.settings["ticks"], head="dialogue"
        )
        loss = nn.functional.cross_entropy(
            logits.flatten(0, 1), y.flatten(), ignore_index=-100
        )
        operation_rows = [r for r in rows if r.get("label") is not None]
        if operation_rows:
            ox, lengths = operation_batch(
                operation_rows, self.codec, self.model.config, self.settings["ticks"]
            )
            target = torch.tensor(
                [
                    self.model.template["labels"].index(r["label"])
                    for r in operation_rows
                ],
                device=self.device,
            )
            pred, _ = self.model(
                ox.to(self.device),
                lengths=lengths.to(self.device),
                readout_every=self.settings["ticks"],
            )
            loss = loss + nn.functional.cross_entropy(pred, target)
        if not torch.isfinite(loss):
            raise RuntimeError("Nonfinite training loss; checkpoint unchanged")
        loss.backward()
        nn.utils.clip_grad_norm_(self.model.parameters(), 1.0, error_if_nonfinite=True)
        self.optimizer.step()
        self.model.constrain()
        self.steps += 1
        return float(loss.detach())

    @torch.no_grad()
    def evaluate(self, rows):
        self.model.eval()
        total_loss = correct = count = 0
        for i in range(0, len(rows), self.settings["batch_size"]):
            x, y = self.tensors(rows[i : i + self.settings["batch_size"]])
            logits, _ = self.model(
                x,
                every_step=True,
                readout_every=self.settings["ticks"],
                head="dialogue",
            )
            total_loss += nn.functional.cross_entropy(
                logits.flatten(0, 1), y.flatten(), ignore_index=-100, reduction="sum"
            ).item()
            valid = y != -100
            correct += ((logits.argmax(-1) == y) & valid).sum().item()
            count += valid.sum().item()
        if not count:
            raise ValueError("Evaluation needs answer tokens")
        return {
            "answer_nll": total_loss / count,
            "token_accuracy": correct / count,
            "answer_tokens": count,
            "scope": "Teacher-forced tokens; not free-conversation accuracy",
        }

    def fit(self, train, dev, *, steps=100, output="run", validate_every=25):
        if not train or not dev or not 1 <= steps <= 100_000 or validate_every < 1:
            raise ValueError(
                "Supply nonempty train/dev, positive steps and validation interval"
            )
        fingerprint = hashlib.sha256(
            json.dumps([train, dev], sort_keys=True).encode()
        ).hexdigest()
        if self.data_fingerprint and fingerprint != self.data_fingerprint:
            raise ValueError(
                "Resume dataset changed; start a new Trainer for a different curriculum"
            )
        self.data_fingerprint = fingerprint
        output = Path(output)
        output.mkdir(parents=True, exist_ok=True)
        if self.best_checkpoint is not None:
            atomic_json(output / "best.json", self.best_checkpoint)
        # Reject invalid/oversized records before modifying any weights.
        for row in train + dev:
            language_batch(
                [row],
                self.codec,
                self.model.config,
                self.settings["ticks"],
                self.settings["max_symbols"],
            )
            if (
                row.get("label") is not None
                and row["label"] not in self.model.template["labels"]
            ):
                raise ValueError(f"Unknown operation label: {row['label']}")

        def validate():
            result = self.evaluate(dev)
            if result["answer_nll"] < self.best_score:
                self.best_score, self.best_step = result["answer_nll"], self.steps
                self.export(output / "best.json")
                self.best_checkpoint = json.loads((output / "best.json").read_text())
            self.history.append({"step": self.steps, **result})
            print(json.dumps(self.history[-1]), flush=True)
            self.save(output / "resume.pt")
            atomic_json(output / "history.json", self.history)

        validate()
        start = time.perf_counter()
        domains = sorted({row.get("domain", "unspecified") for row in train})
        by_domain = {
            d: [r for r in train if r.get("domain", "unspecified") == d]
            for d in domains
        }
        try:
            for i in range(steps):
                rows = [
                    self.rng.choice(by_domain[self.rng.choice(domains)])
                    for _ in range(self.settings["batch_size"])
                ]
                self.update(rows)
                self.replay.extend(copy.deepcopy(rows))
                if (i + 1) % validate_every == 0 or i + 1 == steps:
                    validate()
        finally:
            self.save(output / "resume.pt")
            self.export(output / "latest.json")
        return {
            "steps": self.steps,
            "best_step": self.best_step,
            "seconds": time.perf_counter() - start,
        }

    def sleep(self, steps=10):
        """Offline rehearsal of observed training records, never generated answers."""
        if not self.replay or not 1 <= steps <= 10_000:
            raise ValueError(
                "Sleep requires observed training records and 1..10000 steps"
            )
        losses = []
        for _ in range(steps):
            losses.append(
                self.update(
                    [
                        self.rng.choice(self.replay)
                        for _ in range(self.settings["batch_size"])
                    ]
                )
            )
            self.replay_steps += 1
        return {
            "replay_steps": steps,
            "mean_loss": sum(losses) / len(losses),
            "scope": "Supervised memory replay; no biological sleep claim",
        }

    def export(self, path):
        result = self.model.export(
            {
                "method": "surrogate_bptt",
                "encoder": "ordered_bpe_question_v1",
                "steps": self.steps,
                "replay_steps": self.replay_steps,
                "sensory_codec": "coarse_multisensory_v1",
                "continuous_state": "optional_inference",
            }
        )
        result["dialogue"] = {
            **self.codec.json(),
            "ticks_per_symbol": self.settings["ticks"],
            "fast_readout": self.model.fast_readout,
            "readout": {
                "weights": self.model.dialogue_output.weight.detach().cpu().tolist(),
                "bias": self.model.dialogue_output.bias.detach().cpu().tolist(),
                "updates": self.steps,
            },
        }
        result["updates"] = self.steps
        result["readout"]["updates"] = self.steps
        # A changing core invalidates old operation calibration. Require Rust calibration.
        result["threshold"] = 1.01
        result["gates"] = [
            {"confidence": 1.01, "similarity": -1.0} for _ in result["labels"]
        ]
        result["centroids"] = [[0.0] * self.model.neurons for _ in result["labels"]]
        result["presentations"] = [0] * len(result["labels"])
        atomic_json(Path(path), result)
        return Path(path)

    def save(self, path):
        path = Path(path)
        path.parent.mkdir(parents=True, exist_ok=True)
        payload = {
            "version": 1,
            "template": self.model.template,
            "codec": self.codec.json(),
            "fast_readout": self.model.fast_readout,
            "settings": self.settings,
            "model": {k: v.detach().cpu() for k, v in self.model.state_dict().items()},
            "optimizer": self.optimizer.state_dict(),
            "rng": self.rng.getstate(),
            "steps": self.steps,
            "replay_steps": self.replay_steps,
            "replay": list(self.replay),
            "history": self.history,
            "data_fingerprint": self.data_fingerprint,
            "best_score": self.best_score,
            "best_step": self.best_step,
            "best_checkpoint": self.best_checkpoint,
        }
        temporary = None
        try:
            with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as stream:
                temporary = Path(stream.name)
                torch.save(payload, stream)
                stream.flush()
                os.fsync(stream.fileno())
            os.replace(temporary, path)
        finally:
            if temporary is not None:
                temporary.unlink(missing_ok=True)

    @classmethod
    def resume(cls, path, device="auto"):
        payload = torch.load(path, map_location="cpu", weights_only=True)
        if payload["version"] != 1:
            raise ValueError("Unsupported training checkpoint")
        codec = Tokenizer(**payload["codec"])
        model = initialize(payload["template"], codec, device=device)
        model.fast_readout = payload["fast_readout"]
        model.load_state_dict(payload["model"])
        trainer = cls(model, codec, **payload["settings"])
        trainer.optimizer.load_state_dict(payload["optimizer"])
        trainer.rng.setstate(payload["rng"])
        for key in (
            "steps",
            "replay_steps",
            "history",
            "data_fingerprint",
            "best_score",
            "best_step",
            "best_checkpoint",
        ):
            setattr(trainer, key, payload[key])
        trainer.replay.extend(payload["replay"])
        return trainer


class Session:
    """Stateful neural inference; generated text never becomes a training label."""

    def __init__(self, model, codec, ticks=2, continuous=True):
        if not 1 <= ticks <= 8:
            raise ValueError("ticks must be 1..8")
        self.model, self.codec, self.ticks = model, codec, ticks
        self.continuous = continuous
        self.sequence_decoder=None
        payload=model.template.get('sequence_decoder')
        if payload:
            from .sequence import SequenceDecoder
            device=model.magnitudes.device
            self.sequence_decoder=SequenceDecoder(**payload['settings']).to(device).eval()
            self.sequence_decoder.load_state_dict({name:torch.tensor(value,device=device) for name,value in payload['weights'].items()})
        self.reset()

    def reset(self):
        self.state = None
        self.elapsed_ticks = 0
        self.turns = 0

    @torch.no_grad()
    def feed(self, spikes):
        self.model.eval()
        logits, self.state = self.model(
            spikes.to(self.model.magnitudes.device), state=self.state, head="dialogue"
        )
        self.state = self.state.detach()
        self.elapsed_ticks += spikes.shape[1]
        return logits

    def tokens(self, sequence):
        return self.feed(
            token_spikes(
                torch.tensor([sequence]),
                self.model.inputs,
                self.ticks,
                len(self.codec.tokens),
            )
        )

    def observe(self, observation):
        self.feed(sensory_spikes(observation, self.model.inputs, 16 * self.ticks))
        return self.status()

    def idle(self, ticks=100):
        if not 1 <= ticks <= 10_000:
            raise ValueError("idle ticks must be 1..10000")
        self.feed(torch.zeros(1, ticks, self.model.inputs))
        return self.status()

    @torch.no_grad()
    def ask(self, question, max_tokens=64, observation=None):
        sequence = (
            [len(self.codec.tokens)]
            + self.codec.encode(question)
            + [len(self.codec.tokens) + 1]
        )
        if (
            not question.strip()
            or len(question.encode()) > 8192
            or len(sequence) > (514 if self.sequence_decoder is not None else 512)
            or not 1 <= max_tokens <= 128
        ):
            raise ValueError(
                "Use a nonempty question <=512 symbols and max_tokens 1..128"
            )
        if not self.continuous:
            self.reset()
        if observation is not None:
            self.observe(observation)
        if self.sequence_decoder is not None:
            from .sequence import activity_features
            state=self.model.initial(1) if self.state is None else self.state
            device=self.model.magnitudes.device;source=torch.tensor([sequence[1:-1]],device=device)
            generated=self.sequence_decoder.generate(source,torch.tensor([source.shape[1]],device=device),activity_features(self.model,state),max_tokens)[0].cpu().tolist()
            reason='end_symbol' if 0 in generated else 'length_limit'
            if 0 in generated:generated=generated[:generated.index(0)]
            self.tokens(sequence+generated+[0]);self.turns+=1
            return dict(text=self.codec.decode(generated),tokens=generated,method='learned_attentive_circuit_decoder',stop_reason=reason,continuous=self.continuous,simulated_ms=self.elapsed_ticks*self.model.config['dt_ms'])
        logits = self.tokens(sequence)
        selected = []
        entropies = []
        reason = "length_limit"
        for _ in range(max_tokens):
            probabilities = logits[0].softmax(-1)
            entropies.append(
                float(-(probabilities * probabilities.clamp_min(1e-12).log()).sum())
            )
            token = int(logits[0].argmax())
            if token == 0:
                reason = "end_symbol"
                break
            selected.append(token)
            # Feed every emitted symbol, including the last one, to retain turn state.
            logits = self.tokens([token])
            if len(selected) >= 5 and len(set(selected[-5:])) == 1:
                reason = "repetition_limit"
                break
        if self.continuous:
            self.tokens([0])
        self.turns += 1
        return {
            "text": self.codec.decode(selected),
            "tokens": selected,
            "stop_reason": reason,
            "continuous": self.continuous,
            "simulated_ms": self.elapsed_ticks * self.model.config["dt_ms"],
            "mean_token_entropy": sum(entropies) / max(len(entropies), 1),
        }

    def dream(self, seed="", max_tokens=32):
        # Imagination is isolated from the live session and the training replay buffer.
        imagined = Session(self.model, self.codec, self.ticks)
        imagined.state = copy.deepcopy(self.state)
        result = imagined.ask(seed or "Imagine a scene.", max_tokens=max_tokens)
        return {**result, "kind": "unverified_generation", "learned_as_fact": False}

    def status(self):
        return {
            "turns": self.turns,
            "simulated_ms": self.elapsed_ticks * self.model.config["dt_ms"],
            "mean_voltage": None
            if self.state is None
            else float(self.state.voltage.mean()),
            "active_trace_neurons": 0
            if self.state is None
            else int((self.state.trace > 0.01).sum()),
            "scope": "Numerical activity monitoring; no awareness or subjective-time score",
        }


def load_session(path, device="auto", continuous=True):
    checkpoint = json.loads(Path(path).read_text())
    dialogue = checkpoint.get("dialogue")
    if not dialogue:
        raise ValueError("Checkpoint has no trained text head")
    codec = Tokenizer(dialogue["tokens"], dialogue["merges"])
    model = initialize(checkpoint, codec, device=device)
    return Session(model, codec, dialogue["ticks_per_symbol"], continuous)
