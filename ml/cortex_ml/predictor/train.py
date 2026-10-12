"""Training the predictor and measuring it against "nothing changes".

    uv run python -m cortex_ml.predictor.train --repo path/to/repo --trajectories eval/results

The last fifth of the examples, in time order, is held out. The number that
matters is the held-out distance compared with the baseline that predicts
the file stays as it is. If the model does not beat that, it has learned
nothing useful.
"""

import argparse
from dataclasses import dataclass

import numpy as np
import torch

from cortex_ml.predictor.data import Edit, edits_from_git, edits_from_trajectories
from cortex_ml.predictor.model import LatentPredictor, distance

HELD_OUT_SHARE = 0.2


@dataclass
class Tensors:
    states: torch.Tensor
    actions: torch.Tensor
    targets: torch.Tensor

    # What each example was an attempt at. Examples that share a group are
    # never split between training and held-out data: repeated attempts at
    # one task make nearly the same edit, and scoring on those would only
    # measure memorisation. `None` treats every example as its own group.
    groups: list[str] | None = None

    def __len__(self) -> int:
        return self.states.shape[0]

    def select(self, rows: list[int]) -> "Tensors":
        return Tensors(self.states[rows], self.actions[rows], self.targets[rows])


@dataclass
class Result:
    examples: int
    held_out: int

    # Mean cosine distance on the held-out examples.
    baseline: float
    model: float

    @property
    def beats_baseline(self) -> bool:
        return self.model < self.baseline


def embed_edits(edits: list[Edit], url: str) -> Tensors:
    from cortex_ml.service_client import embed_documents, embed_queries

    states = embed_documents(url, [(edit.path, edit.before) for edit in edits])
    targets = embed_documents(url, [(edit.path, edit.after) for edit in edits])
    actions = embed_queries(url, [edit.action for edit in edits])

    as_tensor = lambda array: torch.from_numpy(np.asarray(array, dtype=np.float32))

    # The action text is the commit's diff of one file, or the task's
    # instruction, so repeated attempts at a task share it.
    groups = [edit.action for edit in edits]

    return Tensors(as_tensor(states), as_tensor(actions), as_tensor(targets), groups)


def fit(model: LatentPredictor, data: Tensors, epochs: int = 200, rate: float = 1e-3) -> None:
    optimizer = torch.optim.AdamW(model.parameters(), lr=rate, weight_decay=1e-2)

    for _ in range(epochs):
        optimizer.zero_grad()

        loss = distance(model(data.states, data.actions), data.targets).mean()

        loss.backward()
        optimizer.step()


def evaluate(model: LatentPredictor, data: Tensors) -> tuple[float, float]:
    """Mean held-out distance of "nothing changes" and of the model."""

    if len(data) == 0:
        return 0.0, 0.0

    with torch.no_grad():
        baseline = distance(data.states, data.targets).mean()
        predicted = distance(model(data.states, data.actions), data.targets).mean()

    return float(baseline), float(predicted)


def train_and_evaluate(data: Tensors, epochs: int = 200, hidden: int = 512) -> tuple[LatentPredictor, Result]:
    """Fit on the earlier examples and score on the later ones."""

    split = int(len(data) * (1.0 - HELD_OUT_SHARE))

    groups = data.groups or [str(row) for row in range(len(data))]

    seen = set(groups[:split])

    # Later examples of a group already trained on are left out entirely.
    held_out_rows = [row for row in range(split, len(data)) if groups[row] not in seen]

    training = data.select(list(range(split)))
    held_out = data.select(held_out_rows)

    model = LatentPredictor(dimension=data.states.shape[1], hidden=hidden)

    fit(model, training, epochs=epochs)

    baseline, predicted = evaluate(model, held_out)

    return model, Result(len(data), len(held_out), baseline, predicted)


def main() -> None:
    parser = argparse.ArgumentParser(description="Train the JEPA-style edit predictor.")

    parser.add_argument("--repo", action="append", default=[], help="a git repository (repeatable)")
    parser.add_argument("--trajectories", action="append", default=[], help="a directory of run logs")
    parser.add_argument("--out", default="predictor.pt")
    parser.add_argument("--epochs", type=int, default=200)
    parser.add_argument("--embedding-url", default="http://127.0.0.1:8765")

    args = parser.parse_args()

    edits: list[Edit] = []

    for repo in args.repo:
        edits.extend(edits_from_git(repo))

    for directory in args.trajectories:
        edits.extend(edits_from_trajectories(directory))

    print(f"{len(edits)} edits")

    if len(edits) < 20:
        print("too few to split into training and held-out sets")

        return

    data = embed_edits(edits, args.embedding_url)

    model, result = train_and_evaluate(data, epochs=args.epochs)

    if result.held_out < 5:
        print(f"only {result.held_out} held-out edits are from tasks not trained on; too few to judge")

        return

    print(f"held out: {result.held_out} of {result.examples} edits")
    print(f"  predicting no change:  distance {result.baseline:.4f}")
    print(f"  the trained predictor: distance {result.model:.4f}")

    if result.beats_baseline:
        print("the predictor beats the baseline on held-out edits")
    else:
        print("the predictor does NOT beat the baseline; it is not saved")

        return

    torch.save({"dimension": model.dimension, "state": model.state_dict()}, args.out)

    print(f"saved {args.out}")


if __name__ == "__main__":
    main()
