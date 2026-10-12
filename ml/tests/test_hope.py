import torch
from torch.nn import functional as F

from cortex_ml.hope.model import HopeConfig, HopeMemory
from cortex_ml.hope.train import Example, NestedOptimizer, loss_for, top_hit_rate, train

DIMENSION = 16


def small_model() -> HopeMemory:
    return HopeMemory(HopeConfig(dimension=DIMENSION, hidden=32, periods=(1, 4, 16)))


def test_an_untrained_model_ranks_like_cosine():
    torch.manual_seed(0)

    model = small_model()

    query = torch.randn(DIMENSION)
    candidates = torch.randn(5, DIMENSION)

    expected = F.normalize(candidates, dim=-1) @ F.normalize(query, dim=-1)

    assert torch.allclose(model.score(query, candidates), expected, atol=1e-5)


def test_levels_update_at_their_own_frequency():
    torch.manual_seed(0)

    model = small_model()
    optimizer = NestedOptimizer(model, rate=1e-2)

    example = Example(
        query=torch.randn(DIMENSION),
        candidates=torch.randn(4, DIMENSION),
        useful=torch.tensor([True, False, False, False]),
    )

    def snapshot():
        return [level.project.weight.detach().clone() for level in model.continuum.levels]

    start = snapshot()

    for _ in range(3):
        loss_for(model, example).backward()
        optimizer.step()

    after_three = snapshot()

    # Only the fastest level has moved after three steps.
    assert not torch.equal(after_three[0], start[0])
    assert torch.equal(after_three[1], start[1])
    assert torch.equal(after_three[2], start[2])

    for _ in range(13):
        loss_for(model, example).backward()
        optimizer.step()

    after_sixteen = snapshot()

    assert not torch.equal(after_sixteen[1], start[1])
    assert not torch.equal(after_sixteen[2], start[2])


def test_training_learns_a_ranking_cosine_gets_wrong():
    torch.manual_seed(0)

    # Tasks and their useful memories live in different directions: the
    # useful memory for a task is a fixed rotation of the task, while a
    # decoy sits close to the task itself. Plain cosine picks the decoy.
    rotation = torch.linalg.qr(torch.randn(DIMENSION, DIMENSION)).Q

    examples = []

    for _ in range(300):
        query = F.normalize(torch.randn(DIMENSION), dim=-1)

        useful = query @ rotation
        decoy = F.normalize(query + 0.3 * torch.randn(DIMENSION), dim=-1)

        examples.append(
            Example(
                query=query,
                candidates=torch.stack([decoy, useful, torch.randn(DIMENSION)]),
                useful=torch.tensor([False, True, False]),
            )
        )

    model = small_model()

    held_out = examples[240:]

    assert top_hit_rate(model, held_out) < 0.2

    history = train(model, examples[:240], epochs=40, rate=3e-3)

    assert history[-1] < history[0]
    assert top_hit_rate(model, held_out) > 0.7


def test_fast_memory_learns_an_association_without_training():
    torch.manual_seed(0)

    model = small_model()

    query = F.normalize(torch.randn(DIMENSION), dim=-1)
    useful = F.normalize(torch.randn(DIMENSION), dim=-1)
    other = F.normalize(torch.randn(DIMENSION), dim=-1)

    candidates = torch.stack([other, useful])

    before = model.score(query, candidates)

    for _ in range(5):
        model.observe(query, useful)

    after = model.score(query, candidates)

    # The memory that proved useful now scores higher for this query.
    assert after[1] > before[1]
    assert after[1] > after[0]

    model.fast.reset()

    assert torch.allclose(model.score(query, candidates), before, atol=1e-5)
