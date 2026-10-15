"""Explicit bounded structural-growth experiments, requiring a retention gate."""

from __future__ import annotations

import copy
import random

from .portable import initialize
from .training import ConversationTrainer, parameter_report


def grow_candidate(trainer, count=32, seed=731):
    model, codec = trainer.model, trainer.codec
    old_n = model.neurons
    if not 1 <= count <= max(1, old_n // 10) or old_n + count > 8192:
        raise ValueError("Growth is limited to 10% per experiment and 8192 neurons")
    if (old_n + count) * len(codec.tokens) > 40_000_000:
        raise ValueError("Growth exceeds the local runtime's output-synapse budget")
    checkpoint = model.export(
        {"method": "surrogate_bptt", "encoder": "ordered_bpe_question_v1"}
    )
    net = checkpoint["network"]
    soma = 4 if net.get("version", 1) == 1 else 255

    def branch_route():
        slot = rng.randrange(len(newborn["branches"]) + 1)
        return soma if slot == len(newborn["branches"]) else slot

    cfg = net["config"]
    if (old_n + count) * model.fan_in > cfg["max_synapses"]:
        raise ValueError("Growth exceeds the saved synapse budget")
    position = cfg["association"]
    rng = random.Random(seed)
    newborn = copy.deepcopy(net["neurons"][0])
    newborn["population"] = "Association"
    newborn["threshold_scale"] = 1.0
    for branch in newborn["branches"]:
        branch["gain"] = 1.0
    net["neurons"][position:position] = [copy.deepcopy(newborn) for _ in range(count)]
    cfg["association"] += count
    inputs = cfg["inputs"]
    for synapse in net["synapses"]:
        if synapse["target"] >= position:
            synapse["target"] += count
        if synapse["source"] >= inputs + position:
            synapse["source"] += count
    for target in range(position, position + count):
        sources = rng.sample(
            [i for i in range(old_n + count) if i != target], cfg["fan_in"]
        )
        for source in sources:
            inhibitory = net["neurons"][source]["population"] == "Inhibitory"
            net["synapses"].append(
                {
                    "source": inputs + source,
                    "target": target,
                    "branch": soma if inhibitory else branch_route(),
                    "magnitude": cfg["inhibitory_weight"] * 0.25
                    if inhibitory
                    else cfg["excitatory_weight"] * 0.25,
                    "delay": rng.randint(1, model.delay_steps),
                }
            )
        for source in rng.sample(range(inputs), cfg["input_fan_in"]):
            net["synapses"].append(
                {
                    "source": source,
                    "target": target,
                    "branch": branch_route(),
                    "magnitude": cfg["input_weight"] * 0.5,
                    "delay": rng.randint(1, model.delay_steps),
                }
            )
    net["synapses"].sort(key=lambda s: s["target"])
    for row in checkpoint["readout"]["weights"]:
        row[position:position] = [0.0] * count
    checkpoint["centroids"] = [[0.0] * (old_n + count) for _ in checkpoint["labels"]]
    checkpoint["presentations"] = [0] * len(checkpoint["labels"])
    weights = model.dialogue_output.weight.detach().cpu().tolist()
    for row in weights:
        row[position:position] = [rng.uniform(-0.001, 0.001) for _ in range(count)]
    checkpoint["dialogue"] = {
        **codec.json(),
        "ticks_per_symbol": trainer.settings["ticks"],
        "fast_readout": model.fast_readout,
        "readout": {
            "weights": weights,
            "bias": model.dialogue_output.bias.detach().cpu().tolist(),
            "updates": trainer.steps,
        },
    }
    candidate = ConversationTrainer(
        initialize(checkpoint, codec, device=str(trainer.device)),
        codec,
        **trainer.settings,
    )
    candidate.steps = trainer.steps
    return candidate


def grow_checked(trainer, observed_windows, retention_windows, count=32, updates=2):
    if not observed_windows or not retention_windows or not 1 <= updates <= 100:
        raise ValueError(
            "Growth requires observed examples and separate retention checks"
        )

    def score(candidate):
        values = [candidate.window_loss(*w) for w in retention_windows]
        return sum(x[0] for x in values) / max(sum(x[2] for x in values), 1)

    before = score(trainer)
    candidate = grow_candidate(trainer, count=count)
    for i in range(updates):
        windows = [
            observed_windows[(i + j) % len(observed_windows)]
            for j in range(candidate.settings["accumulation"])
        ]
        candidate.update(windows)
    after = score(candidate)
    accepted = after <= before
    report = {
        "accepted": accepted,
        "old_neurons": trainer.model.neurons,
        "candidate_neurons": candidate.model.neurons,
        "before_nll": before,
        "after_nll": after,
        "optimizer_restarted_for_new_dimensions": True,
        "candidate_parameters": parameter_report(candidate.model)[
            "trainable_parameters"
        ],
        "scope": "Engineering growth with a narrow retention gate; not biological neurogenesis or proven broad improvement",
    }
    return (candidate if accepted else trainer), report
