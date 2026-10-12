"""Training the HOPE-style memory model on the agent's own memory.

    uv run python -m cortex_ml.hope.train --memory path/to/memory.db --out hope.pt

The training signal comes from what the agent actually did. For every pair
of an earlier and a later task in the same repository, the earlier one
counts as useful to the later one when the later task ended up changing a
file the earlier task had read or changed. The model learns to rank useful
earlier tasks above the others for a new task's embedding.

The stored vectors of earlier tasks are reused; only the task wordings need
embedding, which the caller supplies through `embed_queries`.
"""

import argparse
import json
import sqlite3
from collections.abc import Callable
from dataclasses import dataclass

import numpy as np
import torch
from torch.nn import functional as F

from cortex_ml.hope.model import HopeConfig, HopeMemory

TEMPERATURE = 0.05


@dataclass
class Example:
    """One task, the memories on offer, and which of them were useful."""

    query: torch.Tensor
    candidates: torch.Tensor
    useful: torch.Tensor


class NestedOptimizer:
    """Updates each level of the continuum memory at its own frequency.

    Gradients are collected on every step. A level with period `p` applies
    what it has collected only on every `p`-th step, so the slow levels
    move on the average of many examples and the fast one on each.
    """

    def __init__(self, model: HopeMemory, rate: float = 1e-3):
        self.levels = list(model.continuum.levels)

        self.optimizers = [torch.optim.Adam(level.parameters(), lr=rate) for level in self.levels]

        self.steps = 0

    def step(self) -> None:
        """Call after `loss.backward()`."""

        self.steps += 1

        for level, optimizer in zip(self.levels, self.optimizers):
            if self.steps % level.period != 0:
                continue

            # The collected gradient is a sum over the period; average it.
            for parameter in level.parameters():
                if parameter.grad is not None:
                    parameter.grad /= level.period

            optimizer.step()
            optimizer.zero_grad()


def loss_for(model: HopeMemory, example: Example) -> torch.Tensor:
    """Useful memories should score above the rest (InfoNCE)."""

    scores = model.score(example.query, example.candidates) / TEMPERATURE

    log_probabilities = F.log_softmax(scores, dim=0)

    return -log_probabilities[example.useful].mean()


def train(model: HopeMemory, examples: list[Example], epochs: int = 30, rate: float = 1e-3) -> list[float]:
    """Train in place and return the mean loss of each epoch."""

    optimizer = NestedOptimizer(model, rate)

    history = []

    for _ in range(epochs):
        total = 0.0

        for index in torch.randperm(len(examples)).tolist():
            loss = loss_for(model, examples[index])

            loss.backward()
            optimizer.step()

            total += float(loss.detach())

        history.append(total / max(len(examples), 1))

    return history


def top_hit_rate(model: HopeMemory, examples: list[Example]) -> float:
    """Share of tasks whose best-scored memory is a useful one."""

    if not examples:
        return 0.0

    hits = 0

    with torch.no_grad():
        for example in examples:
            best = int(model.score(example.query, example.candidates).argmax())

            hits += int(bool(example.useful[best]))

    return hits / len(examples)


def examples_from_memory(
    path: str,
    embed_queries: Callable[[list[str]], np.ndarray],
) -> list[Example]:
    """Build training examples from a Cortex memory database."""

    connection = sqlite3.connect(path)

    rows = connection.execute(
        "SELECT scope, body, embedding FROM episodes WHERE embedding IS NOT NULL "
        "ORDER BY scope, created_ms, id"
    ).fetchall()

    connection.close()

    by_scope: dict[str, list[tuple[dict, np.ndarray]]] = {}

    for scope, body, embedding in rows:
        vector = np.frombuffer(embedding, dtype="<f4")

        by_scope.setdefault(scope, []).append((json.loads(body), vector))

    examples = []

    for episodes in by_scope.values():
        for position in range(1, len(episodes)):
            later, _ = episodes[position]
            earlier = episodes[:position]

            changed = set(later["files_changed"])

            useful = [
                bool(changed & (set(episode["files_changed"]) | set(episode["files_read"])))
                for episode, _ in earlier
            ]

            # A task with no useful memory, or only useful ones, has
            # nothing to teach about ranking.
            if not any(useful) or all(useful):
                continue

            query = embed_queries([later["instruction"]])[0]

            examples.append(
                Example(
                    query=torch.from_numpy(np.array(query, dtype=np.float32)),
                    candidates=torch.from_numpy(np.stack([vector for _, vector in earlier])),
                    useful=torch.tensor(useful),
                )
            )

    return examples


def main() -> None:
    from cortex_ml.service_client import embed_queries

    parser = argparse.ArgumentParser(description="Train the HOPE-style memory model.")

    parser.add_argument("--memory", required=True, help="a Cortex memory.db")
    parser.add_argument("--out", default="hope.pt")
    parser.add_argument("--epochs", type=int, default=30)
    parser.add_argument("--embedding-url", default="http://127.0.0.1:8765")

    args = parser.parse_args()

    examples = examples_from_memory(args.memory, lambda texts: embed_queries(args.embedding_url, texts))

    print(f"{len(examples)} tasks with a useful and a not-useful earlier task to rank")

    if len(examples) < 2:
        print("not enough to train on; run more tasks with `--with memory` first")

        return

    model = HopeMemory(HopeConfig(dimension=int(examples[0].query.shape[0])))

    before = top_hit_rate(model, examples)

    history = train(model, examples, epochs=args.epochs)

    after = top_hit_rate(model, examples)

    print(f"loss {history[0]:.3f} -> {history[-1]:.3f}")
    print(f"useful memory ranked first: {before:.0%} (plain cosine) -> {after:.0%} (trained, same tasks)")
    print("these are training-set numbers; they show the model can fit, not that it generalises")

    torch.save({"config": model.config.__dict__, "state": model.state_dict()}, args.out)

    print(f"saved {args.out}")


if __name__ == "__main__":
    main()
