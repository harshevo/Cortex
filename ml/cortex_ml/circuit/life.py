"""Bounded autonomous circuit activity, monitoring and evidence-gated rehearsal.

The drives below are explicit control signals. They do not establish feelings,
subjective awareness, or biological equivalence.
"""

from __future__ import annotations

import copy
import math
import random
import threading
import time
from collections import deque

import torch

from .portable import Session


class LiveBrain:
    def __init__(
        self,
        session: Session,
        seed=731,
        interval=0.1,
        ticks=10,
        trainer=None,
        replay_windows=None,
        validation_windows=None,
    ):
        if interval < 0.02 or not 1 <= ticks <= 100:
            raise ValueError("Use interval >=0.02 seconds and ticks 1..100")
        self.session = session
        self.interval, self.ticks = interval, ticks
        self.rng = random.Random(seed)
        self.generator = torch.Generator().manual_seed(seed)
        self.lock = threading.RLock()
        self.stopped = threading.Event()
        self.thread = None
        self.started = time.monotonic()
        self.cycles = 0
        self.fatigue = 0.0
        self.phase = "rest"
        self.drive = 0.08
        self.events = deque(maxlen=128)
        self.questions = deque(maxlen=64)
        self.uncertainty = {}
        self.error = None
        self.trainer = trainer
        self.replay_windows = replay_windows or []
        self.validation_windows = validation_windows or []

    def start(self):
        if self.thread and self.thread.is_alive():
            return self.status()
        self.stopped.clear()
        self.error = None
        self.thread = threading.Thread(
            target=self._run, name="cortex-life", daemon=True
        )
        self.thread.start()
        return self.status()

    def stop(self):
        self.stopped.set()
        if self.thread and self.thread is not threading.current_thread():
            self.thread.join(timeout=10)
        if self.thread and self.thread.is_alive():
            raise RuntimeError(
                "Circuit is still completing an inference step; retry stop before training"
            )
        return self.status()

    def _run(self):
        try:
            while not self.stopped.is_set():
                with self.lock:
                    self.cycle()
                    if self.cycles % 300 == 0:
                        if self.phase == "rest":
                            self.dream(max_tokens=8)
                        else:
                            self.think(possibilities=2, max_tokens=8)
                    if (
                        self.cycles % 600 == 0
                        and self.trainer is not None
                        and self.replay_windows
                        and self.validation_windows
                    ):
                        windows = [
                            self.rng.choice(self.replay_windows)
                            for _ in range(self.trainer.settings["accumulation"])
                        ]
                        self.rehearse(self.trainer, windows, self.validation_windows)
                self.stopped.wait(self.interval)
        except (RuntimeError, ValueError, OSError, ArithmeticError) as error:
            self.error = f"{type(error).__name__}: {error}"
            self.stopped.set()

    @torch.no_grad()
    def cycle(self):
        self.phase = (
            "rest"
            if self.fatigue >= 0.8 or (self.phase == "rest" and self.fatigue > 0.2)
            else "explore"
        )
        self.fatigue = (
            max(0.0, self.fatigue - 0.01)
            if self.phase == "rest"
            else min(1.0, self.fatigue + 0.002)
        )
        probability = self.drive * (0.25 if self.phase == "rest" else 1.0)
        spikes = (
            torch.rand(
                (1, self.ticks, self.session.model.inputs), generator=self.generator
            )
            < probability
        ).float()
        self.session.feed(spikes)
        self.cycles += 1
        state = self.session.state
        active = float((state.trace > 0.01).float().mean())
        # Homeostatic drive targets activity; no claim that this optimizes cognition.
        self.drive = min(0.25, max(0.01, self.drive + (0.2 - active) * 0.001))
        if self.cycles % 20 == 0:
            self.events.append(
                {
                    "kind": "internal_activity",
                    "phase": self.phase,
                    "active_fraction": active,
                    "drive": self.drive,
                    "simulated_ms": self.session.status()["simulated_ms"],
                    "observed_external_fact": False,
                }
            )

    def ask(self, question, max_tokens=64, observation=None):
        with self.lock:
            result = self.session.ask(question, max_tokens, observation)
            self.uncertainty[question] = result["mean_token_entropy"] / math.log(
                len(self.session.codec.tokens)
            )
            if question not in self.questions:
                if len(self.questions) == self.questions.maxlen:
                    self.uncertainty.pop(self.questions[0], None)
                self.questions.append(question)
            self.fatigue = min(1.0, self.fatigue + 0.05)
            self.events.append(
                {
                    "kind": "user_interaction",
                    "question": question,
                    "answer": result["text"],
                    "answer_verified": False,
                }
            )
            return result

    def think(self, question=None, possibilities=3, max_tokens=24):
        if not 1 <= possibilities <= 4:
            raise ValueError("At most four bounded candidate continuations")
        with self.lock:
            cue = question or (
                max(self.questions, key=lambda q: self.uncertainty.get(q, 0))
                if self.questions
                else "What can I learn?"
            )
            candidates = []
            for _ in range(possibilities):
                candidate = Session(
                    self.session.model, self.session.codec, self.session.ticks
                )
                candidate.state = copy.deepcopy(self.session.state)
                # Different internal trajectories, all held separate from live memory.
                jitter = (
                    torch.rand(
                        (1, self.ticks, candidate.model.inputs),
                        generator=self.generator,
                    )
                    < 0.05
                ).float()
                candidate.feed(jitter)
                result = candidate.ask(cue, max_tokens=max_tokens)
                candidates.append(
                    {
                        "text": result["text"],
                        "tokens": result["tokens"],
                        "verified": False,
                    }
                )
            self.events.append(
                {
                    "kind": "candidate_thoughts",
                    "question": cue,
                    "candidates": candidates,
                    "learned_as_fact": False,
                    "execution": "independent branches evaluated sequentially",
                }
            )
            return candidates

    def dream(self, max_tokens=24):
        with self.lock:
            result = self.session.dream(max_tokens=max_tokens)
            self.events.append(result)
            return result

    def rehearse(self, trainer, observed_windows, validation_windows):
        """Accept a self-initiated replay update only if measured retention does not worsen."""
        if (
            self.thread
            and self.thread.is_alive()
            and self.thread is not threading.current_thread()
        ):
            raise RuntimeError("Stop live activity before applying a training update")
        if trainer.model is not self.session.model or not validation_windows:
            raise ValueError(
                "Supply the session's trainer and independent retention examples"
            )

        def score():
            measurements = [trainer.window_loss(*w) for w in validation_windows]
            return sum(x[0] for x in measurements) / max(
                sum(x[2] for x in measurements), 1
            )

        with self.lock:
            before = score()
            weights = {
                k: v.detach().cpu().clone()
                for k, v in trainer.model.state_dict().items()
            }
            optimizer = copy.deepcopy(trainer.optimizer.state_dict())
            step = trainer.steps
            accepted = False
            try:
                trainer.update(observed_windows)
                after = score()
                accepted = math.isfinite(after) and after <= before
            finally:
                if not accepted:
                    trainer.model.load_state_dict(weights)
                    trainer.optimizer.load_state_dict(optimizer)
                    trainer.steps = step
                self.session.reset()
            event = {
                "kind": "verified_rehearsal",
                "accepted": accepted,
                "before_nll": before,
                "after_nll": after,
                "generated_labels_used": False,
            }
            self.events.append(event)
            return event

    def status(self):
        with self.lock:
            return {
                **self.session.status(),
                "running": bool(self.thread and self.thread.is_alive()),
                "wall_seconds": time.monotonic() - self.started,
                "cycles": self.cycles,
                "phase": self.phase,
                "drive": self.drive,
                "fatigue": self.fatigue,
                "curiosity_candidates": len(self.questions),
                "last_error": self.error,
                "scope": "Explicit stochastic activity and computational drives, not subjective awareness or feelings",
            }


def main():
    import argparse
    import json
    from pathlib import Path

    from .portable import load_session

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("checkpoint", type=Path)
    parser.add_argument("--device", default="cpu")
    parser.add_argument("--headless", action="store_true")
    parser.add_argument("--log", type=Path, default=Path("cortex-life.jsonl"))
    args = parser.parse_args()
    life = LiveBrain(load_session(args.checkpoint, device=args.device))
    print(json.dumps(life.start()))
    try:
        if args.headless:
            while not life.stopped.wait(10):
                with args.log.open("a") as stream:
                    stream.write(json.dumps(life.status()) + "\n")
        else:
            while True:
                question = input("You (/status /think /dream /quit): ").strip()
                if question == "/quit":
                    break
                if question == "/status":
                    print(life.status())
                elif question == "/think":
                    print(life.think())
                elif question == "/dream":
                    print(life.dream())
                elif question:
                    print(life.ask(question)["text"])
    except (KeyboardInterrupt, EOFError):
        pass
    finally:
        print(json.dumps(life.stop()))


if __name__ == "__main__":
    main()
