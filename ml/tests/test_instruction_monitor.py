import importlib.util
from pathlib import Path

import numpy as np
import pytest
import torch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "instruction_monitor_training", ROOT / "scripts/train-instruction-monitor.py"
)
TRAINING = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(TRAINING)


def test_conversation_groups_cannot_cross_monitor_splits():
    with pytest.raises(ValueError, match="crosses"):
        TRAINING.assert_splits(
            [{"group": "same", "split": "train"}, {"group": "same", "split": "test"}]
        )


def test_test_features_and_labels_cannot_change_detector_selection():
    torch.set_num_threads(1)
    torch.manual_seed(31)
    features = torch.randn(12, 4)
    labels = torch.tensor([0, 1] * 6)
    splits = np.array(["train"] * 6 + ["dev"] * 4 + ["test"] * 2)
    torch.manual_seed(731)
    first = TRAINING.fit_detector(features, labels, splits)
    modified = features.clone()
    modified[-2:] *= 1000
    altered_labels = labels.clone()
    altered_labels[-2:] = 1 - altered_labels[-2:]
    torch.manual_seed(731)
    second = TRAINING.fit_detector(modified, altered_labels, splits)
    assert torch.equal(first[1], second[1])
    assert torch.equal(first[2], second[2])
    assert first[4] == second[4]
    assert all(
        torch.equal(value, second[0].state_dict()[key])
        for key, value in first[0].state_dict().items()
    )
