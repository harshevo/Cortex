import json
import subprocess

import torch
from torch.nn import functional as F

from cortex_ml.predictor.data import edits_from_git, edits_from_trajectories
from cortex_ml.predictor.model import LatentPredictor, distance
from cortex_ml.predictor.train import Tensors, train_and_evaluate

DIMENSION = 16


def test_an_untrained_predictor_predicts_no_change():
    torch.manual_seed(0)

    model = LatentPredictor(dimension=DIMENSION, hidden=32)

    states = F.normalize(torch.randn(4, DIMENSION), dim=-1)
    actions = torch.randn(4, DIMENSION)

    assert torch.allclose(model(states, actions), states, atol=1e-6)
    assert distance(states, states).abs().max() < 1e-6


def test_it_learns_changes_that_depend_on_the_action():
    torch.manual_seed(0)

    # The state after an edit is the state before it, moved in a direction
    # the action determines. "No change" is wrong by a constant amount.
    effect = torch.randn(DIMENSION, DIMENSION) * 0.3

    states = F.normalize(torch.randn(400, DIMENSION), dim=-1)
    actions = F.normalize(torch.randn(400, DIMENSION), dim=-1)
    targets = F.normalize(states + actions @ effect, dim=-1)

    _, result = train_and_evaluate(Tensors(states, actions, targets), epochs=300, hidden=64)

    assert result.held_out == 80
    assert result.beats_baseline
    assert result.model < result.baseline * 0.5


def test_edits_are_read_from_git_history(tmp_path):
    def git(*args):
        subprocess.run(
            ["git", "-C", str(tmp_path), "-c", "user.name=t", "-c", "user.email=t@t", *args],
            check=True,
            capture_output=True,
        )

    git("init", "-q")

    (tmp_path / "money.py").write_text("def cents(x):\n    return x\n")
    git("add", ".")
    git("commit", "-q", "-m", "add cents")

    (tmp_path / "money.py").write_text("def cents(x):\n    return round(x * 100)\n")
    (tmp_path / "notes.md").write_text("not code\n")
    git("add", ".")
    git("commit", "-q", "-m", "round to cents")

    edits = edits_from_git(str(tmp_path))

    assert len(edits) == 1
    assert edits[0].path == "money.py"
    assert "return x" in edits[0].before
    assert "round(x * 100)" in edits[0].after
    assert edits[0].action.startswith("round to cents")


def test_edits_are_read_from_trajectories(tmp_path):
    events = [
        {"type": "run_started", "instruction": "round to cents"},
        {
            "type": "tool_started",
            "call": {
                "id": "1",
                "name": "edit_file",
                "input": {"path": "money.py", "old_text": "return x", "new_text": "return round(x)"},
            },
        },
        {"type": "tool_finished", "result": {"call_id": "1", "is_error": False}},
        {
            "type": "tool_started",
            "call": {"id": "2", "name": "edit_file", "input": {"path": "a.py", "old_text": "a", "new_text": "b"}},
        },
        {"type": "tool_finished", "result": {"call_id": "2", "is_error": True}},
    ]

    (tmp_path / "run.jsonl").write_text("\n".join(json.dumps(event) for event in events))

    edits = edits_from_trajectories(str(tmp_path))

    assert len(edits) == 1
    assert edits[0].action == "round to cents"
    assert edits[0].after == "return round(x)"


def test_repeated_attempts_at_a_task_are_not_scored():
    torch.manual_seed(0)

    states = F.normalize(torch.randn(10, DIMENSION), dim=-1)
    actions = F.normalize(torch.randn(10, DIMENSION), dim=-1)

    # The last two examples repeat a task that is in the training part.
    groups = [f"task {row}" for row in range(8)] + ["task 0", "task 1"]

    _, result = train_and_evaluate(Tensors(states, actions, states, groups), epochs=1, hidden=8)

    assert result.held_out == 0
