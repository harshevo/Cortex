"""Training the HOPE-style memory model on public issue data.

    uv run python -m cortex_ml.hope.pretrain --examples data/hope-examples.npz --out hope.pt

Whole repositories are set aside before training: some to choose the
model's size and when to stop, and some that are looked at once, at the
end. The weights are saved only if, on those last repositories, the model
ranks useful earlier tasks better than plain cosine similarity does.
"""

import argparse
import copy
import hashlib
import time

import torch

from cortex_ml.hope.model import HopeConfig, HopeMemory
from cortex_ml.hope.public import load_examples
from cortex_ml.hope.train import Example, NestedOptimizer, loss_for


def split(by_repo: dict[str, list[Example]]) -> tuple[list[Example], list[Example], list[Example]]:
    """Training, validation and test examples, no repository in two of
    them. A repository's place depends only on its name."""

    parts: tuple[list[Example], list[Example], list[Example]] = ([], [], [])

    for repo, examples in by_repo.items():
        bucket = hashlib.sha256(repo.encode()).digest()[0] % 20

        parts[0 if bucket < 14 else 1 if bucket < 17 else 2].extend(examples)

    return parts


def quality(model: HopeMemory, examples: list[Example]) -> tuple[float, float]:
    """How well useful memories are ranked: the share of tasks whose
    best-scored memory is useful, and the mean of one over the rank of the
    first useful memory."""

    hits, reciprocal = 0, 0.0

    with torch.no_grad():
        for example in examples:
            order = model.score(example.query, example.candidates).argsort(descending=True)

            first = int(example.useful[order].nonzero()[0])

            hits += int(first == 0)
            reciprocal += 1.0 / (first + 1)

    count = max(len(examples), 1)

    return hits / count, reciprocal / count


def on(device: str, examples: list[Example]) -> list[Example]:
    return [
        Example(example.query.to(device), example.candidates.to(device), example.useful.to(device))
        for example in examples
    ]


def fit(
    config: HopeConfig,
    training: list[Example],
    validation: list[Example],
    rate: float,
    epochs: int,
    patience: int = 3,
    device: str = "cpu",
) -> tuple[HopeMemory, float]:
    """The model as it was after its best epoch on the validation
    repositories, and its score there."""

    torch.manual_seed(0)

    model = HopeMemory(config).to(device)

    model.fast.matrix = model.fast.matrix.to(device)

    optimizer = NestedOptimizer(model, rate)

    best, best_state = quality(model, validation)[1], copy.deepcopy(model.state_dict())
    stale = 0

    for epoch in range(epochs):
        for index in torch.randperm(len(training)).tolist():
            loss_for(model, training[index]).backward()

            optimizer.step()

        score = quality(model, validation)[1]

        print(f"  epoch {epoch + 1}: validation {score:.4f}", flush=True)

        if score > best:
            best, best_state, stale = score, copy.deepcopy(model.state_dict()), 0
        else:
            stale += 1

            if stale == patience:
                break

    model.load_state_dict(best_state)

    return model, best


def main() -> None:
    parser = argparse.ArgumentParser(description="Train the HOPE-style memory model on public issue data.")

    parser.add_argument("--examples", default="data/hope-examples.npz", help="file written by cortex_ml.hope.public")
    parser.add_argument("--out", default="hope.pt")
    parser.add_argument("--hidden", type=int, nargs="+", default=[256])

    # Rates of 3e-4 and above, and a hidden width of 1024, made ranking in
    # unseen repositories worse than plain cosine after one epoch.
    parser.add_argument("--rate", type=float, nargs="+", default=[3e-5, 1e-5])
    parser.add_argument("--epochs", type=int, default=8)
    parser.add_argument("--device", default="cpu", help="cpu or mps; one small example a step is quicker on cpu")

    args = parser.parse_args()

    training, validation, test = (on(args.device, part) for part in split(load_examples(args.examples)))

    print(f"tasks to rank: {len(training)} training, {len(validation)} validation, {len(test)} test")

    dimension = int(training[0].query.shape[0])

    chosen, chosen_score = None, -1.0

    for hidden in args.hidden:
        for rate in args.rate:
            started = time.time()

            print(f"hidden {hidden}, rate {rate}:", flush=True)

            model, score = fit(
                HopeConfig(dimension=dimension, hidden=hidden), training, validation, rate, args.epochs, device=args.device
            )

            print(f"  best validation {score:.4f} in {time.time() - started:.0f} s", flush=True)

            if score > chosen_score:
                chosen, chosen_score = model, score

    untrained = HopeMemory(HopeConfig(dimension=dimension)).to(args.device)

    untrained.fast.matrix = untrained.fast.matrix.to(args.device)

    cosine_first, cosine_rank = quality(untrained, test)
    first, rank = quality(chosen, test)

    parameters = sum(parameter.numel() for parameter in chosen.parameters())

    print(f"chosen: hidden {chosen.config.hidden}, {parameters:,} parameters")
    print(f"repositories never trained on, {len(test)} tasks:")
    print(f"  useful memory ranked first: {cosine_first:.1%} (plain cosine) -> {first:.1%} (trained)")
    print(f"  mean reciprocal rank:       {cosine_rank:.4f} (plain cosine) -> {rank:.4f} (trained)")

    if first <= cosine_first or rank <= cosine_rank:
        print("not better than plain cosine; nothing saved")

        return

    torch.save({"config": chosen.config.__dict__, "state": chosen.cpu().state_dict()}, args.out)

    print(f"saved {args.out}")


if __name__ == "__main__":
    main()
