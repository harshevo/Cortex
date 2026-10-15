import json
import subprocess
from pathlib import Path

import pytest
import torch
from cortex_ml.circuit.data import dialogue_batch, operation_batch, word_spikes
from cortex_ml.circuit.model import Circuit, Spike
from cortex_ml.circuit.tokenizer import Tokenizer, dialogue_tokens

ROOT = Path(__file__).resolve().parents[2]


@pytest.fixture(scope="module")
def checkpoint(tmp_path_factory):
    binary = ROOT / "target/release/cortex-brain-circuit"
    if not binary.exists():
        pytest.skip("Build the circuit CLI first")
    path = tmp_path_factory.mktemp("circuit") / "initial.json"
    subprocess.run([str(binary), "init", str(path), "small"], check=True)
    return json.loads(path.read_text())


def test_byte_pairs_roundtrip_and_vocabulary_uses_only_supplied_text():
    codec = Tokenizer.fit(["A small brain learns.", "The brain learns words."], 280)
    for text in ["A small brain learns.", "Unseen Ω words, 12 + 3."]:
        assert codec.decode(codec.encode(text)) == text
    assert len(codec.tokens) <= 280
    assert len(codec.merges) == len(codec.tokens) - 257


def test_teacher_forcing_never_inputs_the_next_answer_token():
    codec = Tokenizer.fit(["hello", "world"], 270)
    row = {"question": "hello", "answer": "world"}
    sequence, target = dialogue_tokens(row, codec)
    n = len(codec.encode("hello")) + 1
    assert all(t == -100 for t in target[:n])
    assert sequence[n] == len(codec.tokens) + 1
    assert target[n:] == codec.encode("world") + [0]
    assert target[-1] == 0


def test_spike_surrogate_has_documented_derivative():
    x = torch.tensor([-0.2, 0, 0.2], requires_grad=True)
    y = Spike.apply(x)
    y.sum().backward()
    assert y.tolist() == [0, 1, 1]
    assert torch.allclose(x.grad, 1 / (1 + 5 * x.detach().abs()).square())


def test_rust_and_tensor_forward_agree(checkpoint, tmp_path):
    torch.set_num_threads(1)
    path = tmp_path / "model.json"
    path.write_text(json.dumps(checkpoint))
    model = Circuit(checkpoint)
    text = "Find force for mass 3 kg and acceleration 4 m/s^2."
    oracle = json.loads(
        subprocess.check_output(
            [
                str(ROOT / "target/release/cortex-brain-circuit"),
                "probe",
                str(path),
                text,
            ]
        )
    )
    with torch.no_grad():
        logits, state = model(word_spikes([text], model.config))
    assert torch.allclose(
        model.features(state)[0], torch.tensor(oracle["features"]), atol=1e-4
    )
    assert torch.allclose(
        logits.softmax(-1)[0], torch.tensor(oracle["probabilities"]), atol=1e-5
    )


def test_all_requested_parameter_groups_receive_gradients(checkpoint):
    torch.manual_seed(13)
    model = Circuit(checkpoint)
    x = word_spikes(
        ["Find force for mass 3 kg and acceleration 4 m/s^2.", "What is 1 + 3?"],
        model.config,
    )
    logits, _ = model(x)
    torch.nn.functional.cross_entropy(logits, torch.tensor([4, 0])).backward()
    # A zero initialized head needs its first update before recurrent credit arrives.
    with torch.no_grad():
        model.output.weight.normal_(std=0.05)
    model.zero_grad(set_to_none=True)
    logits, _ = model(x)
    torch.nn.functional.cross_entropy(logits, torch.tensor([4, 0])).backward()
    for gradient in [
        model.magnitudes.grad[model.recurrent_mask],
        model.magnitudes.grad[model.dendritic_mask],
        model.branch_gain.grad,
        model.threshold_scale.grad,
    ]:
        assert torch.isfinite(gradient).all()
        assert gradient.abs().max() > 0
    before = model.magnitudes.detach().clone()
    optimizer = torch.optim.Adam(model.parameters(), lr=0.001)
    optimizer.step()
    model.constrain()
    assert (
        before[model.recurrent_mask] != model.magnitudes[model.recurrent_mask]
    ).any()
    assert (
        before[model.dendritic_mask] != model.magnitudes[model.dendritic_mask]
    ).any()
    assert ((model.magnitudes >= 0) & (model.magnitudes <= 15)).all()


def test_numerical_substitutions_are_invariant_but_word_order_is_visible(checkpoint):
    codec = Tokenizer.fit(
        ["Sam gives Mira coins. Mira gives Sam coins. What is # + #?"], 300
    )
    rows = [{"question": "What is 2 + 3?"}, {"question": "What is 42 + 19?"}]
    x, lengths = operation_batch(rows, codec, checkpoint["network"]["config"], 2)
    assert lengths[0] == lengths[1]
    assert torch.equal(x[0], x[1])
    a, _ = operation_batch(
        [{"question": "Sam gives Mira coins."}],
        codec,
        checkpoint["network"]["config"],
        2,
    )
    b, _ = operation_batch(
        [{"question": "Mira gives Sam coins."}],
        codec,
        checkpoint["network"]["config"],
        2,
    )
    assert not torch.equal(a, b)


def test_padding_does_not_change_operation_features(checkpoint):
    codec = Tokenizer.fit(["short", "a much longer question with many words"], 300)
    model = Circuit(checkpoint)
    short = [{"question": "short"}]
    together = short + [{"question": "a much longer question with many words"}]
    a, la = operation_batch(short, codec, model.config, 2)
    b, lb = operation_batch(together, codec, model.config, 2)
    with torch.no_grad():
        fa, _ = model(a, lengths=la, readout_every=2, head="features")
        fb, _ = model(b, lengths=lb, readout_every=2, head="features")
    assert torch.allclose(fa[0], fb[0], atol=1e-5)


def test_frozen_control_really_preserves_recurrent_and_intrinsic_parameters(checkpoint):
    model = Circuit(checkpoint).frozen_recurrence()
    before = [
        p.detach().clone()
        for p in [model.magnitudes, model.branch_gain, model.threshold_scale]
    ]
    optimizer = torch.optim.Adam(
        [p for p in model.parameters() if p.requires_grad], lr=0.01
    )
    logits, _ = model(word_spikes(["hello", "good morning"], model.config))
    torch.nn.functional.cross_entropy(logits, torch.tensor([0, 1])).backward()
    optimizer.step()
    assert all(
        torch.equal(a, b)
        for a, b in zip(
            before, [model.magnitudes, model.branch_gain, model.threshold_scale]
        )
    )


def test_language_and_operation_losses_use_the_same_recurrent_parameters(checkpoint):
    codec = Tokenizer.fit(["hello there", "hello friend"], 280)
    model = Circuit(checkpoint, dialogue_vocab=len(codec.tokens))
    x, y = dialogue_batch(
        [{"question": "hello", "answer": "hello friend"}], codec, model.config, 2
    )
    logits, _ = model(x, every_step=True, readout_every=2, head="dialogue")
    torch.nn.functional.cross_entropy(
        logits.flatten(0, 1), y.flatten(), ignore_index=-100
    ).backward()
    assert (
        model.magnitudes.grad is not None
        and torch.isfinite(model.magnitudes.grad).all()
    )
    assert model.dialogue_output.weight.grad.abs().max() > 0
    assert model.output.weight.grad is None


@pytest.mark.parametrize("fast_readout", [False, True])
def test_trained_ordered_encoder_exports_to_rust_and_restores_both_heads(
    checkpoint, tmp_path, fast_readout
):
    codec = Tokenizer.fit(
        [
            "What is # + #?",
            "Find force for mass # kg and acceleration # m/s^#.",
            "hello world",
        ],
        290,
    )
    model = Circuit(checkpoint, dialogue_vocab=len(codec.tokens))
    model.fast_readout = fast_readout
    with torch.no_grad():
        model.threshold_scale[0] = 0.7
        model.branch_gain[0, 1] = 1.2
        model.magnitudes.add_(0.01)
        model.constrain()
    metadata = {"method": "surrogate_bptt", "encoder": "ordered_bpe_question_v1"}
    result = model.export(metadata)
    result["dialogue"] = {
        **codec.json(),
        "ticks_per_symbol": 2,
        "fast_readout": fast_readout,
        "readout": {
            "weights": model.dialogue_output.weight.detach().tolist(),
            "bias": model.dialogue_output.bias.detach().tolist(),
            "updates": 1,
        },
    }
    path = tmp_path / "trained.json"
    path.write_text(json.dumps(result))
    text = "What is 17 + 32?"
    oracle = json.loads(
        subprocess.check_output(
            [
                str(ROOT / "target/release/cortex-brain-circuit"),
                "probe",
                str(path),
                text,
            ]
        )
    )
    x, lengths = operation_batch([{"question": text}], codec, model.config, 2)
    with torch.no_grad():
        features, _ = model(x, lengths=lengths, readout_every=2, head="features")
    assert torch.allclose(features[0], torch.tensor(oracle["features"]), atol=2e-4)
    restored = Circuit(result, dialogue_vocab=len(codec.tokens))
    assert torch.equal(model.magnitudes, restored.magnitudes)
    assert torch.equal(model.output.weight, restored.output.weight)
    assert torch.equal(model.dialogue_output.weight, restored.dialogue_output.weight)
    assert model.fast_readout == restored.fast_readout
    from cortex_ml.circuit.evaluate import generate

    expected = generate(model, codec, text, 2, "cpu", max_tokens=64)
    actual = json.loads(
        subprocess.check_output(
            [
                str(ROOT / "target/release/cortex-brain-circuit"),
                "generate-neural",
                str(path),
                text,
            ]
        )
    )
    assert expected["tokens"] == actual["tokens"]
    assert expected["stop_reason"] == actual["stop_reason"]


def test_variable_branches_and_legacy_soma_keep_rust_parity(checkpoint, tmp_path):
    import copy

    varied = copy.deepcopy(checkpoint)
    net = varied["network"]
    assert net["version"] == 2
    net["neurons"][0]["branches"].append(
        copy.deepcopy(net["neurons"][0]["branches"][0])
    )
    net["neurons"][1]["branches"] = net["neurons"][1]["branches"][:3]
    for edge in net["synapses"]:
        if edge["target"] == 0 and edge["branch"] == 0:
            edge["branch"] = 4
        if edge["target"] == 1 and edge["branch"] == 3:
            edge["branch"] = 255
    path = tmp_path / "varied.json"
    path.write_text(json.dumps(varied))
    model = Circuit(varied)
    text = "hello friend"
    oracle = json.loads(
        subprocess.check_output(
            [
                str(ROOT / "target/release/cortex-brain-circuit"),
                "probe",
                str(path),
                text,
            ]
        )
    )
    with torch.no_grad():
        _, state = model(word_spikes([text], model.config))
    assert torch.allclose(
        model.features(state)[0], torch.tensor(oracle["features"]), atol=1e-4
    )
    exported = model.export({})
    assert [len(n["branches"]) for n in exported["network"]["neurons"][:2]] == [5, 3]
    legacy = copy.deepcopy(checkpoint)
    legacy["network"]["version"] = 1
    for edge in legacy["network"]["synapses"]:
        if edge["branch"] == 255:
            edge["branch"] = 4
    a, b = Circuit(checkpoint), Circuit(legacy)
    x = word_spikes([text], a.config)
    with torch.no_grad():
        _, sa = a(x)
        _, sb = b(x)
    assert torch.allclose(a.features(sa), b.features(sb), atol=1e-6)
