"""A JEPA-style predictor of what an edit does to code.

A JEPA (joint-embedding predictive architecture) predicts the embedding of
a future state rather than the state itself. Here:

- the state is a file, embedded by the frozen embedding model;
- the action is the description of a change, embedded the same way;
- the prediction is the embedding the file will have after the change.

Predicting in embedding space is far cheaper than generating the edited
file, which is the point: the agent could ask "what would this change do?"
before paying to make and test it.

The predictor outputs a correction to the current state, and starts by
predicting no change. That is the baseline it has to beat: most edits are
small, so "the file stays as it is" is already a good guess.
"""

import torch
from torch import nn
from torch.nn import functional as F


class LatentPredictor(nn.Module):

    def __init__(self, dimension: int = 768, hidden: int = 512):
        super().__init__()

        self.dimension = dimension

        self.network = nn.Sequential(
            nn.Linear(dimension * 2, hidden),
            nn.GELU(),
            nn.Linear(hidden, hidden),
            nn.GELU(),
            nn.Linear(hidden, dimension),
        )

        # Predict "no change" until trained.
        nn.init.zeros_(self.network[-1].weight)
        nn.init.zeros_(self.network[-1].bias)

    def forward(self, states: torch.Tensor, actions: torch.Tensor) -> torch.Tensor:
        """Predicted unit-length embeddings of the states after the actions."""

        change = self.network(torch.cat([states, actions], dim=-1))

        return F.normalize(states + change, dim=-1)


def distance(predicted: torch.Tensor, actual: torch.Tensor) -> torch.Tensor:
    """Cosine distance per row: 0 is a perfect prediction."""

    return 1.0 - F.cosine_similarity(predicted, actual, dim=-1)
