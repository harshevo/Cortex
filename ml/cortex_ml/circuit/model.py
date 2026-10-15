"""Sparse delayed ALIF circuit with hard spikes and explicit surrogate derivatives.

Forward equations mirror cortex-brain's Rust simulator. The reset, refractory
counter and plateau duration use detached hard events. The plateau indicator's
backward derivative uses the local branch voltage's sigmoid derivative. This is
an approximate BPTT estimator, not an exact gradient of the discontinuous model.
"""

from __future__ import annotations

import copy
import math
from dataclasses import dataclass

import torch
from torch import nn


class Spike(torch.autograd.Function):
    @staticmethod
    def forward(ctx, voltage):
        ctx.save_for_backward(voltage)
        return (voltage >= 0).to(voltage.dtype)

    @staticmethod
    def backward(ctx, gradient):
        (voltage,) = ctx.saved_tensors
        return gradient / (1.0 + 5.0 * voltage.abs()).square()


@dataclass
class State:
    voltage: torch.Tensor
    currents: torch.Tensor
    branch_voltage: torch.Tensor
    adaptation: torch.Tensor
    refractory: torch.Tensor
    trace: torch.Tensor
    fast_trace: torch.Tensor
    plateau: torch.Tensor
    armed: torch.Tensor
    history: list[torch.Tensor]

    def detach(self):
        return State(
            **{
                name: [x.detach() for x in value]
                if isinstance(value, list)
                else value.detach()
                for name, value in vars(self).items()
            }
        )


class Circuit(nn.Module):
    def __init__(
        self,
        checkpoint: dict,
        classes: int | None = None,
        dialogue_vocab: int | None = None,
    ):
        super().__init__()
        self.template = copy.deepcopy(
            {k: v for k, v in checkpoint.items() if k != "dialogue"}
        )
        snap = checkpoint["network"]
        self.config = cfg = snap["config"]
        self.inputs = cfg["inputs"]
        self.neurons = len(snap["neurons"])
        self.delay_steps = int(cfg["max_delay_ms"] / cfg["dt_ms"])
        synapses = snap["synapses"]
        self.fan_in = cfg["fan_in"] + cfg["input_fan_in"]
        if not (
            0 < self.neurons <= 8192
            and 0 < self.fan_in <= 256
            and len(synapses) == self.neurons * self.fan_in
        ):
            raise ValueError("Unsupported circuit dimensions for the bounded trainer")
        if [s["target"] for s in synapses] != [
            n for n in range(self.neurons) for _ in range(self.fan_in)
        ]:
            raise ValueError(
                "Incoming connections must retain canonical target ordering"
            )
        source = torch.tensor([s["source"] for s in synapses], dtype=torch.long)
        inhibitory = torch.tensor(
            [n["population"] == "Inhibitory" for n in snap["neurons"]]
        )
        source_inhibitory = torch.cat(
            [torch.zeros(self.inputs, dtype=torch.bool), inhibitory]
        )[source]
        self.register_buffer("sources", source.reshape(self.neurons, self.fan_in))
        self.register_buffer(
            "history_index",
            torch.tensor(
                [self.delay_steps - s["delay"] for s in synapses], dtype=torch.long
            ).reshape(self.neurons, self.fan_in),
        )
        self.branch_slots = max(len(n["branches"]) for n in snap["neurons"])
        if not 2 <= self.branch_slots <= 8:
            raise ValueError("Circuit branches must fit the 2..8 compartment budget")
        version = snap.get("version", 1)
        branch_index = []
        for edge in synapses:
            branch = edge["branch"]
            soma = branch == (4 if version == 1 else 255)
            if not soma and not 0 <= branch < len(
                snap["neurons"][edge["target"]]["branches"]
            ):
                raise ValueError("Synapse targets an absent dendritic branch")
            branch_index.append(self.branch_slots if soma else branch)
        branch_index = torch.tensor(branch_index, dtype=torch.long)
        self.register_buffer(
            "compartments",
            (branch_index + (self.branch_slots + 1) * source_inhibitory.long()).reshape(
                self.neurons, self.fan_in
            ),
        )
        self.register_buffer("excitatory", (~inhibitory).float())
        self.register_buffer(
            "branch_mask",
            torch.tensor(
                [
                    [
                        j < len(n["branches"]) and n["population"] != "Inhibitory"
                        for j in range(self.branch_slots)
                    ]
                    for n in snap["neurons"]
                ],
                dtype=torch.float32,
            ),
        )
        self.register_buffer(
            "recurrent_mask", (source >= self.inputs).reshape(self.neurons, self.fan_in)
        )
        self.register_buffer(
            "dendritic_mask",
            (branch_index < self.branch_slots).reshape(self.neurons, self.fan_in),
        )
        self.magnitudes = nn.Parameter(
            torch.tensor(
                [s["magnitude"] for s in synapses], dtype=torch.float32
            ).reshape(self.neurons, self.fan_in)
        )
        self.threshold_scale = nn.Parameter(
            torch.tensor([n.get("threshold_scale", 1.0) for n in snap["neurons"]])
        )
        self.branch_gain = nn.Parameter(
            torch.tensor(
                [
                    [b.get("gain", 1.0) for b in n["branches"]]
                    + [1.0] * (self.branch_slots - len(n["branches"]))
                    for n in snap["neurons"]
                ]
            )
        )
        outputs = classes or len(checkpoint["readout"]["weights"])
        self.output = nn.Linear(self.neurons, outputs)
        if classes is None:
            with torch.no_grad():
                self.output.weight.copy_(torch.tensor(checkpoint["readout"]["weights"]))
                self.output.bias.copy_(torch.tensor(checkpoint["readout"]["bias"]))
        self.fast_readout = (
            checkpoint["dialogue"].get("fast_readout", False)
            if checkpoint.get("dialogue")
            else True
        )
        self.dialogue_output = (
            nn.Linear(self.neurons, dialogue_vocab) if dialogue_vocab else None
        )
        if dialogue_vocab and checkpoint.get("dialogue"):
            with torch.no_grad():
                self.dialogue_output.weight.copy_(
                    torch.tensor(checkpoint["dialogue"]["readout"]["weights"])
                )
                self.dialogue_output.bias.copy_(
                    torch.tensor(checkpoint["dialogue"]["readout"]["bias"])
                )
        self.decay = {
            name: math.exp(-cfg["dt_ms"] / cfg[name + "_ms"])
            for name in [
                "membrane",
                "excitation",
                "inhibition",
                "branch",
                "adaptation",
                "trace",
            ]
        }

    def initial(self, batch: int):
        def zeros(*shape):
            return self.magnitudes.new_zeros((batch,) + shape)

        return State(
            zeros(self.neurons),
            zeros(self.neurons, 2 * (self.branch_slots + 1)),
            zeros(self.neurons, self.branch_slots),
            zeros(self.neurons),
            zeros(self.neurons),
            zeros(self.neurons),
            zeros(self.neurons),
            zeros(self.neurons, self.branch_slots),
            torch.ones(
                (batch, self.neurons, self.branch_slots),
                device=self.magnitudes.device,
                dtype=torch.bool,
            ),
            [zeros(self.inputs + self.neurons) for _ in range(self.delay_steps)],
        )

    def step(self, external: torch.Tensor, state: State):
        cfg, d = self.config, self.decay
        slots, compartments = self.branch_slots, self.branch_slots + 1
        past = torch.stack(state.history)
        delayed = past[self.history_index, :, self.sources].permute(2, 0, 1)
        weighted = delayed * self.magnitudes.unsqueeze(0)
        arrivals = state.currents.new_zeros(state.currents.shape).scatter_add(
            2,
            self.compartments.unsqueeze(0).expand(external.shape[0], -1, -1),
            weighted,
        )
        excitation = (
            state.currents[:, :, :compartments] + arrivals[:, :, :compartments]
        ) * d["excitation"]
        inhibition = (
            state.currents[:, :, compartments:] + arrivals[:, :, compartments:]
        ) * d["inhibition"]
        currents = torch.cat([excitation, inhibition], dim=2)
        branch_voltage = (
            state.branch_voltage * d["branch"]
            + (excitation[:, :, :slots] - inhibition[:, :, :slots]) * (1 - d["branch"])
        ) * self.branch_mask[None, :, :]
        armed = state.armed | (branch_voltage.detach() < cfg["branch_threshold"] * 0.7)
        trigger = armed & (branch_voltage.detach() >= cfg["branch_threshold"])
        plateau = torch.where(
            trigger,
            torch.full_like(state.plateau, cfg["plateau_ms"]),
            (state.plateau - cfg["dt_ms"]).clamp_min(0),
        )
        soft = torch.sigmoid(5 * (branch_voltage - cfg["branch_threshold"]))
        active = (plateau > 0).to(soft.dtype) + (soft - soft.detach())
        branch_current = (
            cfg["branch_coupling"]
            * (
                self.branch_gain
                * self.branch_mask
                * (branch_voltage.relu() + cfg["plateau_current"] * active)
            ).sum(dim=2)
            * self.excitatory
        )
        adaptation = state.adaptation * d["adaptation"]
        predicted = state.voltage * d["membrane"] + (
            excitation[:, :, slots]
            - inhibition[:, :, slots]
            + branch_current
            - adaptation
        ) * (1 - d["membrane"])
        available = (state.refractory <= 0).to(predicted.dtype)
        spikes = (
            Spike.apply(predicted - cfg["threshold"] * self.threshold_scale) * available
        )
        voltage = predicted * (1 - spikes.detach()) * available
        refractory = torch.where(
            spikes.detach() > 0,
            torch.full_like(state.refractory, cfg["refractory_ms"]),
            (state.refractory - cfg["dt_ms"]).clamp_min(0),
        )
        adaptation = adaptation + cfg["adaptation_step"] * spikes * self.excitatory
        trace = state.trace * d["trace"] + spikes
        fast_trace = state.fast_trace * d["inhibition"] + spikes
        history = state.history[1:] + [torch.cat([external, spikes], dim=1)]
        return State(
            voltage,
            currents,
            branch_voltage,
            adaptation,
            refractory,
            trace,
            fast_trace,
            plateau,
            armed & ~trigger,
            history,
        )

    @staticmethod
    def normalize(trace):
        features = trace / 10
        features = features - features.mean(dim=-1, keepdim=True)
        return (
            5
            * features
            / features.square().sum(dim=-1, keepdim=True).clamp_min(1e-12).sqrt()
        )

    @staticmethod
    def features(state: State):
        return Circuit.normalize(state.trace)

    def forward(
        self,
        external: torch.Tensor,
        state: State | None = None,
        every_step=False,
        readout_every=1,
        head="operation",
        lengths=None,
    ):
        state = self.initial(external.shape[0]) if state is None else state
        traces = []
        for tick in range(external.shape[1]):
            state = self.step(external[:, tick], state)
            if (every_step or lengths is not None) and (tick + 1) % readout_every == 0:
                traces.append(
                    state.fast_trace + 0.1 * state.trace
                    if head == "dialogue" and self.fast_readout
                    else state.trace
                )
        if every_step:
            features = self.normalize(torch.stack(traces, dim=1))
        elif lengths is not None:
            trace = torch.stack(traces, dim=1)[
                torch.arange(external.shape[0], device=external.device), lengths - 1
            ]
            features = self.normalize(trace)
        else:
            features = (
                self.normalize(state.fast_trace + 0.1 * state.trace)
                if head == "dialogue" and self.fast_readout
                else self.features(state)
            )
        if head == "features":
            return features, state
        output = self.output if head == "operation" else self.dialogue_output
        if output is None:
            raise ValueError("No dialogue output head configured")
        return output(features), state

    @torch.no_grad()
    def constrain(self):
        self.magnitudes.clamp_(0, 15)
        self.threshold_scale.clamp_(0.5, 2)
        self.branch_gain.clamp_(0.1, 4)
        self.output.weight.clamp_(-20, 20)
        self.output.bias.clamp_(-20, 20)
        if self.dialogue_output is not None:
            self.dialogue_output.weight.clamp_(-20, 20)
            self.dialogue_output.bias.clamp_(-20, 20)

    def frozen_recurrence(self):
        for p in [self.magnitudes, self.threshold_scale, self.branch_gain]:
            p.requires_grad_(False)
        return self

    @torch.no_grad()
    def export(self, metadata: dict):
        result = copy.deepcopy(self.template)
        weights = self.magnitudes.detach().cpu().flatten().tolist()
        for synapse, weight in zip(result["network"]["synapses"], weights):
            synapse["magnitude"] = weight
        thresholds = self.threshold_scale.detach().cpu().tolist()
        gains = self.branch_gain.detach().cpu().tolist()
        for i, neuron in enumerate(result["network"]["neurons"]):
            for key in [
                "voltage",
                "excitation",
                "inhibition",
                "adaptation",
                "refractory_left_ms",
                "trace",
                "fast_trace",
            ]:
                neuron[key] = 0.0
            neuron["spikes"] = 0
            neuron["threshold_scale"] = thresholds[i]
            for j, branch in enumerate(neuron["branches"]):
                branch.update(
                    excitation=0.0,
                    inhibition=0.0,
                    voltage=0.0,
                    plateau_left_ms=0.0,
                    armed=True,
                    gain=gains[i][j],
                )
        result["network"].update(ticks=0, dropped_events=0)
        result["network"]["queues"] = [[] for _ in range(self.delay_steps + 1)]
        result["readout"]["weights"] = self.output.weight.detach().cpu().tolist()
        result["readout"]["bias"] = self.output.bias.detach().cpu().tolist()
        result["plasticity"] = metadata
        return result
