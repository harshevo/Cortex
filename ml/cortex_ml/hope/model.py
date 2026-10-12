"""A small HOPE-style memory model that re-scores memories for a task.

HOPE is the architecture from Google's Nested Learning paper. No code or
weights were released, so this is a reimplementation of its two ideas at the
scale this project needs. It is not a reproduction of the paper's model.

1. A continuum memory system. Instead of one network trained at one speed,
   a chain of blocks each updated at its own frequency. The fastest block
   changes after every example and tracks what is going on right now; the
   slowest changes rarely and keeps what stays true.

2. A memory that rewrites itself while it is used. A fast associative
   matrix is written at inference time, with no backpropagation through the
   rest of the model, using the "surprise" rule from Titans: the more wrong
   the memory was about an association, the harder it is written.

The model takes the embedding of a task and returns a transformed query.
Stored memories are scored by their similarity to that query. Every block
starts as the identity, so an untrained model ranks exactly as plain cosine
similarity does. It can therefore be switched on before it has learned
anything without making retrieval worse.
"""

from dataclasses import dataclass

import torch
from torch import nn
from torch.nn import functional as F


@dataclass(frozen=True)
class HopeConfig:
    dimension: int = 768
    hidden: int = 256

    # How many training steps pass between updates of each block, fastest
    # first.
    periods: tuple[int, ...] = (1, 4, 16)

    # The fast memory's write rule: step size, momentum of surprise, and
    # how quickly old associations fade.
    fast_rate: float = 0.5
    fast_momentum: float = 0.6
    fast_decay: float = 0.02


class Level(nn.Module):
    """One block of the continuum memory: a residual two-layer network."""

    def __init__(self, dimension: int, hidden: int, period: int):
        super().__init__()

        self.period = period

        self.expand = nn.Linear(dimension, hidden)
        self.project = nn.Linear(hidden, dimension)

        # Start as "add nothing", so the whole chain starts as the identity.
        nn.init.zeros_(self.project.weight)
        nn.init.zeros_(self.project.bias)

    def forward(self, vectors: torch.Tensor) -> torch.Tensor:
        return vectors + self.project(F.gelu(self.expand(vectors)))


class ContinuumMemory(nn.Module):
    """A chain of blocks that learn at different speeds."""

    def __init__(self, config: HopeConfig):
        super().__init__()

        self.levels = nn.ModuleList(
            Level(config.dimension, config.hidden, period) for period in config.periods
        )

    def forward(self, vectors: torch.Tensor) -> torch.Tensor:
        for level in self.levels:
            vectors = level(vectors)

        return vectors


class FastMemory:
    """An associative matrix written while the model is in use.

    `write(key, value)` makes `read(key)` move towards `value`. Nothing
    here is trained by backpropagation; the update is one closed-form step
    on the error of the matrix's own prediction.
    """

    def __init__(self, config: HopeConfig):
        self.config = config

        self.matrix = torch.zeros(config.dimension, config.dimension)
        self.surprise = torch.zeros(config.dimension, config.dimension)

    def read(self, keys: torch.Tensor) -> torch.Tensor:
        return keys @ self.matrix.T

    def write(self, key: torch.Tensor, value: torch.Tensor) -> None:
        key = F.normalize(key.detach(), dim=-1)
        value = value.detach()

        # Gradient of 0.5 * |M key - value|^2 with respect to M.
        error = self.matrix @ key - value
        gradient = torch.outer(error, key)

        config = self.config

        self.surprise = config.fast_momentum * self.surprise - config.fast_rate * gradient
        self.matrix = (1.0 - config.fast_decay) * self.matrix + self.surprise

    def reset(self) -> None:
        self.matrix.zero_()
        self.surprise.zero_()


class HopeMemory(nn.Module):

    def __init__(self, config: HopeConfig | None = None):
        super().__init__()

        self.config = config or HopeConfig()

        self.continuum = ContinuumMemory(self.config)
        self.fast = FastMemory(self.config)

    def transform(self, queries: torch.Tensor) -> torch.Tensor:
        """Turn task embeddings into the queries memories are scored against."""

        slow = self.continuum(queries)

        # What the fast memory adds is the difference between what it
        # recalls and the task itself, so an empty memory adds nothing.
        recalled = self.fast.read(F.normalize(queries, dim=-1))

        return F.normalize(slow + recalled, dim=-1)

    def score(self, query: torch.Tensor, candidates: torch.Tensor) -> torch.Tensor:
        """Similarity of each candidate memory to one task, higher is better."""

        transformed = self.transform(query.unsqueeze(0)).squeeze(0)

        return F.normalize(candidates, dim=-1) @ transformed

    def observe(self, query: torch.Tensor, useful_memory: torch.Tensor) -> None:
        """Learn during a task, without training: a memory proved useful
        for this query, so pull similar queries towards it."""

        target = F.normalize(useful_memory, dim=-1) - F.normalize(query, dim=-1)

        self.fast.write(query, target)
