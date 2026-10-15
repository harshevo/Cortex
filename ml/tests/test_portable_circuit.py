import copy
import json
import subprocess
from pathlib import Path

import pytest
import torch
from cortex_ml.circuit.data import sha256
from cortex_ml.circuit.portable import (
    Session,
    Trainer,
    initialize,
    language_batch,
    load_dataset,
    load_session,
    select_device,
)
from cortex_ml.circuit.senses import sensory_spikes
from cortex_ml.circuit.tokenizer import Tokenizer

ROOT = Path(__file__).resolve().parents[2]


@pytest.fixture(scope="module")
def seed(tmp_path_factory):
    torch.set_num_threads(1)
    binary = ROOT / "target/release/cortex-brain-circuit"
    if not binary.exists():
        pytest.skip("Build the current Rust circuit CLI for interoperability tests")
    path = tmp_path_factory.mktemp("portable") / "initial.json"
    subprocess.run([str(binary), "init", str(path), "small"], check=True)
    return json.loads(path.read_text())


def trainer(seed):
    rows = [
        {"question": "Hi", "answer": "Hello", "group": "greeting", "domain": "chat"},
        {
            "question": "2 + 3?",
            "answer": "5",
            "group": "sum",
            "domain": "maths",
            "label": "(a+b)",
        },
    ]
    codec = Tokenizer.fit(
        [r[k] for r in rows for k in ("question", "answer")], size=280
    )
    model = initialize(seed, codec, device="cpu")
    return Trainer(model, codec, batch_size=1), rows


def test_dataset_loader_rejects_checksum_and_group_leakage(tmp_path):
    for split in ("train", "dev", "test"):
        (tmp_path / f"{split}.jsonl").write_text(
            json.dumps(
                {"question": split, "answer": "unit-test fixture", "group": split}
            )
            + "\n"
        )
    manifest = {"sha256": {p.name: sha256(p) for p in tmp_path.glob("*.jsonl")}}
    (tmp_path / "manifest.json").write_text(json.dumps(manifest))
    load_dataset(tmp_path)
    path = tmp_path / "dev.jsonl"
    path.write_text((tmp_path / "train.jsonl").read_text())
    with pytest.raises(ValueError, match="checksum"):
        load_dataset(tmp_path)
    manifest["sha256"][path.name] = sha256(path)
    (tmp_path / "manifest.json").write_text(json.dumps(manifest))
    with pytest.raises(ValueError, match="leakage"):
        load_dataset(tmp_path)


def test_senses_preserve_spatial_structure_and_reject_bad_inputs():
    a = sensory_spikes({"image": [[1, 0], [0, 0]]})
    b = sensory_spikes({"image": [[0, 1], [0, 0]]})
    assert a.shape == (1, 16, 512) and not torch.equal(a, b)
    assert not a[:, :, :272].any()
    left = sensory_spikes({"points": [[-1, 0, 0]]})
    right = sensory_spikes({"points": [[1, 0, 0]]})
    assert not torch.equal(left, right)
    wave = torch.sin(torch.arange(1024) * 0.2)
    sound = sensory_spikes({"audio": wave, "sample_rate": 16000})
    assert sound[:, :, 400:464].any()
    assert set(sound.unique().tolist()) <= {0, 1}
    with pytest.raises(ValueError, match="16000"):
        sensory_spikes({"audio": wave, "sample_rate": 8000})
    with pytest.raises(ValueError):
        sensory_spikes({"points": [[float("nan"), 0, 0]]})


def test_episode_teacher_forcing_and_sensory_padding(seed):
    trained, rows = trainer(seed)
    episode = {"turns": rows, "observation": {"points": [[1, 0, 0]]}}
    x, y = language_batch([episode, rows[0]], trained.codec, trained.model.config, 2)
    assert (y[0, :16] == -100).all()
    assert x[0, :32, 336:400].any()
    assert (
        y[1, y[1] != -100] == torch.tensor(trained.codec.encode("Hello") + [0])
    ).all()
    with pytest.raises(ValueError, match="limit"):
        language_batch([episode], trained.codec, trained.model.config, 2, max_symbols=8)


def test_optimizer_sampling_and_best_checkpoint_resume(seed, tmp_path):
    trained, rows = trainer(seed)
    before = trained.model.magnitudes.detach().clone()
    trained.fit(rows, [rows[0]], steps=2, output=tmp_path / "first", validate_every=1)
    assert not torch.equal(before, trained.model.magnitudes)
    resumed = Trainer.resume(tmp_path / "first/resume.pt", device="cpu")
    assert resumed.rng.random() == trained.rng.random()
    assert resumed.steps == trained.steps
    assert resumed.best_checkpoint is not None
    trained.update(rows)
    resumed.update(rows)
    for key, expected in trained.model.state_dict().items():
        assert torch.equal(expected, resumed.model.state_dict()[key]), key
    assert (
        resumed.optimizer.state_dict()["param_groups"]
        == trained.optimizer.state_dict()["param_groups"]
    )
    resumed.fit(rows, [rows[0]], steps=1, output=tmp_path / "second", validate_every=1)
    assert (tmp_path / "second/best.json").exists()
    with pytest.raises(ValueError, match="dataset changed"):
        resumed.fit(rows[:1], rows[:1], steps=1, output=tmp_path / "changed")


def test_dreams_and_inference_do_not_train_or_pollute_replay(seed):
    trained, rows = trainer(seed)
    trained.replay.extend(copy.deepcopy(rows))
    session = Session(trained.model, trained.codec)
    before = {k: v.clone() for k, v in trained.model.state_dict().items()}
    session.ask("Hi", max_tokens=3)
    state = copy.deepcopy(session.state)
    clock = session.elapsed_ticks
    result = session.dream("Hi", max_tokens=3)
    assert result["learned_as_fact"] is False
    assert session.elapsed_ticks == clock
    assert torch.equal(state.voltage, session.state.voltage)
    assert list(trained.replay) == rows
    session.idle(10)
    assert session.elapsed_ticks == clock + 10
    assert all(
        torch.equal(value, trained.model.state_dict()[k]) for k, value in before.items()
    )
    trained.sleep(1)
    assert trained.steps == trained.replay_steps == 1
    assert list(trained.replay) == rows


def test_portable_export_runs_in_rust_with_continuous_multisensory_parity(
    seed, tmp_path
):
    trained, rows = trainer(seed)
    trained.update(rows)
    path = trained.export(tmp_path / "trained.json")
    fresh = load_session(path, device="cpu", continuous=False)
    oracle = json.loads(
        subprocess.check_output(
            [
                str(ROOT / "target/release/cortex-brain-circuit"),
                "generate",
                str(path),
                "Hi",
            ]
        )
    )
    assert fresh.ask("Hi")["tokens"] == oracle["tokens"]
    session = load_session(path, device="cpu", continuous=True)
    observation = {"points": [[1, 0, 0]]}
    frames = sensory_spikes(observation, session.model.inputs, 16 * session.ticks)[0]
    request = {
        "question": "Hi",
        "frames": [f.nonzero().flatten().tolist() for f in frames],
    }
    output = subprocess.check_output(
        [str(ROOT / "target/release/cortex-brain-circuit"), "stream", str(path)],
        input=json.dumps(request) + "\n2 + 3?\n/reset\nHi\n/quit\n",
        text=True,
    )
    answers = [json.loads(line) for line in output.splitlines()]
    for rust, question, sensor in [
        (answers[0], "Hi", observation),
        (answers[1], "2 + 3?", None),
    ]:
        python = session.ask(question, observation=sensor)
        assert rust["tokens"] == python["tokens"]
        assert rust["simulated_ms"] == python["simulated_ms"]
    assert answers[2] == {"reset": True}
    session.reset()
    assert session.ask("Hi")["tokens"] == answers[3]["tokens"]


def test_device_selection_does_not_silently_ignore_an_unavailable_gpu(monkeypatch):
    monkeypatch.setattr(torch.cuda, "is_available", lambda: False)
    monkeypatch.setattr(torch.backends.mps, "is_available", lambda: False)
    assert select_device() == "cpu"
    with pytest.raises(ValueError, match="unavailable"):
        select_device("cuda")
