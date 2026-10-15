import json
import subprocess
import time
from pathlib import Path

import pytest
import torch
from cortex_ml.circuit.data import token_channels
from cortex_ml.circuit.growth import grow_candidate, grow_checked
from cortex_ml.circuit.life import LiveBrain
from cortex_ml.circuit.portable import initialize
from cortex_ml.circuit.tokenizer import Tokenizer, dialogue_tokens
from cortex_ml.circuit.training import (
    ConversationTrainer,
    TokenDataset,
    parameter_report,
)

ROOT = Path(__file__).resolve().parents[2]


@pytest.fixture
def trainer(tmp_path):
    torch.set_num_threads(1)
    path = tmp_path / "seed.json"
    subprocess.run(
        [str(ROOT / "target/release/cortex-brain-circuit"), "init", str(path), "small"],
        check=True,
    )
    codec = Tokenizer.fit_fast(["hello there", "a little conversation"], size=270)
    model = initialize(json.loads(path.read_text()), codec, device="cpu")
    return ConversationTrainer(model, codec, chunk=4)


def window(trainer):
    x, y = dialogue_tokens({"question": "hello", "answer": "there"}, trainer.codec)
    return torch.tensor(x), torch.tensor(y)


def test_fast_codec_and_extended_sensor_bits_do_not_collide():
    codec = Tokenizer.fit_fast(["Hello world. नमस्ते दुनिया. Hello again."] * 10, size=300)
    for text in ("Hello world.", "नमस्ते दुनिया.", "unseen\nΩ\ttext"):
        expected = [b + 1 for b in text.encode()]
        for i, pair in enumerate(codec.merges):
            expected = codec.merge(expected, pair, 257 + i)
        assert codec.fast_backend().encode(text).ids == expected
        assert codec.decode(expected) == text
    assert token_channels(1, 8192) != token_channels(1025, 8192)
    assert token_channels(1, 8192) != token_channels(4097, 8192)
    assert max(token_channels(8191, 8192)) < 160


def test_truncated_training_resume_restores_optimizer_exactly(trainer, tmp_path):
    weights = trainer.model.magnitudes.detach().clone()
    sample = window(trainer)
    trainer.update([sample])
    assert not torch.equal(weights, trainer.model.magnitudes)
    path = trainer.save(tmp_path / "step.pt")
    restored = ConversationTrainer.load(path)
    trainer.update([sample])
    restored.update([sample])
    for key, value in trainer.model.state_dict().items():
        assert torch.equal(value, restored.model.state_dict()[key]), key
    assert parameter_report(trainer.model)["trainable_parameters"] > 0


def test_idle_loop_advances_without_questions_and_stops_cleanly(trainer):
    life = LiveBrain(trainer.session(), interval=0.02, ticks=3)
    life.start()
    deadline = time.monotonic() + 2
    while life.cycles < 2 and time.monotonic() < deadline:
        time.sleep(0.01)
    state = life.stop()
    assert state["cycles"] >= 2 and state["simulated_ms"] >= 6
    assert not state["running"] and state["last_error"] is None
    before = life.session.elapsed_ticks
    assert len(life.think("hello", possibilities=2, max_tokens=2)) == 2
    assert life.session.elapsed_ticks == before
    assert life.events[-1]["learned_as_fact"] is False


@pytest.mark.parametrize("evaluation_error", [False, True])
def test_failed_rehearsal_rolls_back_weights_and_optimizer(
    trainer, monkeypatch, evaluation_error
):
    life = LiveBrain(trainer.session())
    sample = window(trainer)
    before = {k: v.clone() for k, v in trainer.model.state_dict().items()}
    real = trainer.window_loss
    calls = 0

    def scored(*args, **kwargs):
        nonlocal calls
        if kwargs.get("backward"):
            return real(*args, **kwargs)
        calls += 1
        if calls > 1 and evaluation_error:
            raise RuntimeError("evaluation interrupted")
        return (1.0 if calls == 1 else 2.0), 0, 1

    monkeypatch.setattr(trainer, "window_loss", scored)
    if evaluation_error:
        with pytest.raises(RuntimeError, match="evaluation interrupted"):
            life.rehearse(trainer, [sample], [sample])
    else:
        result = life.rehearse(trainer, [sample], [sample])
        assert result["accepted"] is False
    assert trainer.steps == 0
    assert all(torch.equal(v, trainer.model.state_dict()[k]) for k, v in before.items())
    assert not trainer.optimizer.state


def test_growth_remaps_indices_and_exports_valid_rust_checkpoint(trainer, tmp_path):
    candidate = grow_candidate(trainer, count=8)
    assert candidate.model.neurons == trainer.model.neurons + 8
    path = candidate.export_rust(tmp_path / "grown.json")
    result = json.loads(
        subprocess.check_output(
            [
                str(ROOT / "target/release/cortex-brain-circuit"),
                "generate",
                str(path),
                "hello",
            ]
        )
    )
    assert "tokens" in result
    with pytest.raises(ValueError, match="10%"):
        grow_candidate(trainer, count=1000)
    chosen, report = grow_checked(
        trainer, [window(trainer)], [window(trainer)], count=4, updates=1
    )
    assert chosen is trainer or report["accepted"]


def test_real_prepared_corpus_is_disjoint_and_uses_no_added_examples():
    directory = ROOT / "target/aikosh-conversation-data"
    if not directory.exists():
        pytest.skip("Prepare the user's conversation corpus first")
    manifest = json.loads((directory / "manifest.json").read_text())
    assert manifest["synthetic_examples_added"] == 0
    groups = {}
    for split in ("train", "dev", "test"):
        data = TokenDataset(directory, split, window=512)
        assert len(data) > 0
        rows = json.loads((directory / f"{split}.index.json").read_text())
        groups[split] = {r["group"] for r in rows}
        assert all(r["source"] in {"dolly-15k", "oasst1", "oasst2"} for r in rows)
    assert not groups["train"] & (groups["dev"] | groups["test"])
    assert not groups["dev"] & groups["test"]
