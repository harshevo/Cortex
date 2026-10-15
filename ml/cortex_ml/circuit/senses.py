"""Bounded experimental sensory codecs; these are not learned perception."""

from __future__ import annotations

import torch
from torch.nn import functional as F


def sensory_spikes(observation: dict, inputs: int = 512, ticks: int = 16):
    """Encode a grayscale image, normalized point cloud and/or audio waveform.

    Text occupies channels below 260. Images use 272:336, occupancy grids
    336:400, and audio 400:464. Deterministic rate coding avoids RNG drift.
    Inputs are deliberately coarse and require paired training to be useful.
    """
    if inputs < 464 or not 1 <= ticks <= 256:
        raise ValueError("Sensory coding requires >=464 inputs and 1..256 ticks")
    rates = torch.zeros(inputs)
    used = False
    for name in ("image", "points", "audio"):
        if name not in observation:
            continue
        used = True
        value = torch.as_tensor(observation[name], dtype=torch.float32)
        if (
            not value.numel()
            or value.numel() > 1_000_000
            or not torch.isfinite(value).all()
        ):
            raise ValueError(f"Invalid or oversized {name} input")
        if name == "image":
            if value.ndim != 2 or value.min() < 0 or value.max() > 1:
                raise ValueError("image must be a 2D grayscale array in [0, 1]")
            rates[272:336] = F.adaptive_avg_pool2d(value[None, None], (8, 8)).flatten()
        elif name == "points":
            if (
                value.ndim != 2
                or value.shape[1] != 3
                or value.min() < -1
                or value.max() > 1
            ):
                raise ValueError("points must be an N x 3 array in [-1, 1]")
            grid = ((value + 1) * 2).long().clamp(0, 3)
            cells = grid[:, 0] * 16 + grid[:, 1] * 4 + grid[:, 2]
            rates[336:400] = torch.bincount(cells, minlength=64).clamp_max(1).float()
        else:
            if value.ndim != 1 or value.numel() < 64 or value.abs().max() > 1:
                raise ValueError("audio must contain >=64 mono samples in [-1, 1]")
            if observation.get("sample_rate") != 16000:
                raise ValueError("Resample audio to 16000 Hz before encoding")
            # Eight temporal windows preserve a little order; eight frequency bands each.
            frames = F.adaptive_avg_pool1d(value[None, None], 1024).reshape(8, 128)
            spectrum = torch.fft.rfft(frames * torch.hann_window(128)).abs()
            bands = F.adaptive_avg_pool1d(spectrum[:, None], 8).squeeze(1).log1p()
            rates[400:464] = (bands / bands.max().clamp_min(1e-8)).flatten()
    if not used:
        raise ValueError("Provide image, points, or audio")
    phase = torch.arange(ticks + 1, dtype=torch.float32)[:, None] * rates
    return (phase[1:].floor() - phase[:-1].floor())[None]
