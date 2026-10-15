#!/usr/bin/env python3
"""Package the user's conversation corpus and a 30M+ circuit notebook for AIKosh."""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import shutil
import subprocess
import textwrap
import zipfile
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def build(data, output):
    data, output = Path(data), Path(output)
    manifest = json.loads((data / "manifest.json").read_text())
    if manifest.get("synthetic_examples_added") != 0 or manifest["vocabulary"] != 8192:
        raise ValueError(
            "Package requires the prepared real conversation corpus and 8192-symbol codec"
        )
    output.mkdir(parents=True, exist_ok=True)
    stage = ROOT / "target/aikosh-runtime"
    stage.mkdir(parents=True, exist_ok=True)
    binary = ROOT / "target/release/cortex-brain-circuit"
    subprocess.run(
        [str(binary), "init", str(stage / "initial-30m.json"), "30m"], check=True
    )
    subprocess.run(
        [str(binary), "init", str(stage / "initial-small.json"), "small"], check=True
    )
    payload_files = {"cortex_ml/__init__.py": b""}
    for name in (
        "__init__.py",
        "model.py",
        "data.py",
        "tokenizer.py",
        "benchmark.py",
        "train.py",
        "portable.py",
        "senses.py",
        "training.py",
        "life.py",
        "growth.py",
    ):
        payload_files[f"cortex_ml/circuit/{name}"] = (
            ROOT / "ml/cortex_ml/circuit" / name
        ).read_bytes()
    for name in ("initial-30m.json", "initial-small.json"):
        payload_files[name] = (stage / name).read_bytes()
    payload = zlib.compress(
        json.dumps(
            {k: base64.b64encode(v).decode() for k, v in payload_files.items()},
            sort_keys=True,
        ).encode(),
        9,
    )
    cells = []

    def add(kind, source):
        item = {
            "cell_type": kind,
            "id": f"cortex-{len(cells):02d}",
            "metadata": {},
            "source": textwrap.dedent(source).strip() + "\n",
        }
        if kind == "code":
            item.update(execution_count=None, outputs=[])
        cells.append(item)

    add(
        "markdown",
        f"""
    # Cortex: 30M+ conversation training on AIKosh / C-DAC

    **Your corpus only. No authored or synthetic examples were added.** Sources:
    Dolly-15k, OASST1, OASST2 from your `research_training_data/datasets/04_conversation`.
    Prepared episodes: **{manifest["counts"]["train"]["episodes"]:,} train /
    {manifest["counts"]["dev"]["episodes"]:,} development / {manifest["counts"]["test"]["episodes"]:,} test**.
    Training tokens: **{manifest["counts"]["train"]["tokens"]:,}**, with complete text
    retained during preparation. Source cards, licenses, checksums and filtering
    counts are included. Upstream validation and duplicate conversation groups do
    not cross into training. Maths present in those conversations is retained;
    no separate synthetic maths curriculum is mixed in.

    Upload this notebook and `conversation_data.zip` into the same AIKosh folder.
    Select a **Python 3.11+ PyTorch CUDA kernel on your A100**. Your 2 TB pooled file
    storage holds datasets and checkpoints. The separate **5 GB GPU-memory budget**
    is enforced for the PyTorch allocator with context headroom and measured below.
    CUDA context/library allocations outside PyTorch are not covered by its allocator
    limit; the provider GPU monitor remains the final whole-process check.

    The preset has 4,096 simulated neurons and an 8,192-token head, totaling over
    30 million trainable parameters. Most parameters are **output synapses**, not
    recurrent brain connections. This does not imply a biologically equivalent
    brain or guarantee fluent conversation, reasoning or awareness.
    """,
    )
    add(
        "markdown",
        """
    ## 1. Kernel and package check
    Use the provider's GPU-enabled PyTorch build. If only a small dependency is
    missing, run `%pip install numpy tokenizers` in a separate cell. Do not replace
    a working provider PyTorch installation. No API keys or pretrained model downloads.
    """,
    )
    add(
        "code",
        """
    import os, sys, json, platform, hashlib, base64, zlib, zipfile, shutil
    from pathlib import Path
    os.environ.setdefault("PYTORCH_ENABLE_MPS_FALLBACK", "0")
    if sys.version_info < (3, 11):
        raise RuntimeError("Select Python 3.11 or newer.")
    import torch, numpy, tokenizers
    from IPython.display import display, FileLink
    torch.set_num_threads(min(4, os.cpu_count() or 1))
    print({"python": platform.python_version(), "torch": str(torch.__version__),
           "cuda": torch.cuda.is_available(), "gpu": torch.cuda.get_device_name(0) if torch.cuda.is_available() else None})
    """,
    )
    add(
        "markdown",
        """
    ## 2. Unpack source and the actual dataset
    Existing modified runtime files are preserved and reported. Extracting requires
    about 200 MB for the prepared corpus plus checkpoint storage. Each full Adam
    checkpoint is roughly 400 MB; all numbered checkpoints are retained for download.
    """,
    )
    source = """
    PAYLOAD = "EMBEDDED_PAYLOAD"
    blob = base64.b64decode(PAYLOAD)
    assert hashlib.sha256(blob).hexdigest() == "PAYLOAD_DIGEST"
    ASSETS = Path.cwd() / "cortex_runtime"
    ASSETS.mkdir(exist_ok=True)
    for relative, encoded in json.loads(zlib.decompress(blob)).items():
        path = (ASSETS / relative).resolve()
        if not path.is_relative_to(ASSETS.resolve()):
            raise ValueError("Invalid runtime path")
        content = base64.b64decode(encoded)
        if path.exists() and path.read_bytes() != content:
            raise RuntimeError(f"Modified runtime preserved: {path}. Use a new folder for this notebook version.")
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content)
    DATA = Path.cwd() / "conversation_data"
    if not (DATA / "manifest.json").exists():
        archive = Path.cwd() / "conversation_data.zip"
        if not archive.exists():
            raise FileNotFoundError("Upload conversation_data.zip beside this notebook.")
        with zipfile.ZipFile(archive) as bundle:
            for item in bundle.infolist():
                destination = (Path.cwd() / item.filename).resolve()
                if not destination.is_relative_to(DATA.resolve()):
                    raise ValueError("Unexpected dataset archive path")
            bundle.extractall(Path.cwd())
    sys.path.insert(0, str(ASSETS))
    from cortex_ml.circuit.training import ConversationTrainer, TokenDataset, gpu_budget, parameter_report
    from cortex_ml.circuit.portable import initialize
    from cortex_ml.circuit.tokenizer import Tokenizer
    from cortex_ml.circuit.life import LiveBrain
    print(json.loads((DATA / "manifest.json").read_text())["counts"])
    """.replace("EMBEDDED_PAYLOAD", base64.b64encode(payload).decode()).replace(
        "PAYLOAD_DIGEST", hashlib.sha256(payload).hexdigest()
    )
    add("code", source)
    add(
        "markdown",
        """
    ## 3. Memory-bounded 30M+ configuration
    Microbatch is one sequence. Gradient accumulation simulates a larger batch.
    Only `CHUNK_SYMBOLS` tokens retain a gradient graph at once; recurrent state is
    carried forward but detached at chunk boundaries (**truncated BPTT**). This limits
    credit assignment across long delays. Windows preserve all answer-bearing parts
    of the corpus; state resets between training windows, not between inference turns.

    CUDA allocation cap: 4.25 GB, leaving 0.75 GB of the 5 GB budget for context and
    libraries. No large GPU model copy or mixed-precision spike approximation is used.
    If an out-of-memory error occurs, restart the kernel and reduce `CHUNK_SYMBOLS`
    from 16 to 8 or 4. The notebook reports actual peaks; local CPU checks cannot
    certify an untested provider's A100 memory use.
    """,
    )
    add(
        "code",
        """
    DEVICE = "cuda"                  # Use "cpu" only for local checks/inference.
    GPU_MEMORY_GB = 5.0
    STEPS = 1000                      # Additional optimizer updates; increase for longer training.
    CHECKPOINT_EVERY = 100
    CHUNK_SYMBOLS = 16
    WINDOW_SYMBOLS = 512
    ACCUMULATION = 1
    DEV_WINDOWS = 32
    RESUME = None                     # e.g. "runs/aikosh/step-00000100.pt"
    RUN = Path.cwd() / "runs" / "aikosh"
    SMOKE = os.environ.get("CORTEX_NOTEBOOK_SMOKE") == "1"
    if SMOKE:
        DEVICE, STEPS, CHECKPOINT_EVERY, CHUNK_SYMBOLS, WINDOW_SYMBOLS, DEV_WINDOWS = "cpu", 1, 1, 4, 16, 1
        RUN = Path.cwd() / "runs" / "smoke"
    if DEVICE == "cuda" and not torch.cuda.is_available():
        raise RuntimeError("No CUDA GPU is visible. Select the AIKosh GPU kernel/allocation first.")
    print(gpu_budget(GPU_MEMORY_GB) if DEVICE == "cuda" else {"device": DEVICE, "gpu_budget_tested": False})
    codec = Tokenizer(**json.loads((DATA / "tokenizer.json").read_text()))
    train_data = TokenDataset(DATA, "train", window=WINDOW_SYMBOLS)
    dev_data = TokenDataset(DATA, "dev", window=WINDOW_SYMBOLS)
    if RESUME:
        trainer = ConversationTrainer.load(RESUME, device=DEVICE)
    else:
        seed_file = "initial-small.json" if SMOKE else "initial-30m.json"
        model = initialize(json.loads((ASSETS / seed_file).read_text()), codec, device=DEVICE)
        trainer = ConversationTrainer(model, codec, chunk=CHUNK_SYMBOLS, accumulation=ACCUMULATION)
    architecture = parameter_report(trainer.model)
    print(json.dumps(architecture, indent=2))
    if not SMOKE:
        assert architecture["trainable_parameters"] >= 30_000_000
    RUN.mkdir(parents=True, exist_ok=True)
    """,
    )
    add(
        "markdown",
        """
    ## 4. Measure a real optimizer step before the long run
    This warm-up trains on the first training window and records the update. It
    allocates Adam's optimizer state, so the peak includes weights, gradients,
    optimizer and the actual circuit graph. It never uses development/test answers.
    Saved/resumed optimizer updates are retained. Whole-process GPU memory can be
    checked in the AIKosh monitor or with `nvidia-smi` if the provider exposes it.
    """,
    )
    add(
        "code",
        """
    if not RESUME:
        warmup_loss = trainer.update([train_data[0]] * trainer.settings["accumulation"])
        print({"warmup_answer_nll": warmup_loss})
    if DEVICE == "cuda":
        torch.cuda.synchronize()
        peak = torch.cuda.max_memory_reserved()
        print({"peak_allocated_GB": torch.cuda.max_memory_allocated()/1e9, "peak_reserved_GB": peak/1e9})
        assert peak <= 4.25e9, "Allocator budget exceeded; reduce CHUNK_SYMBOLS and restart."
    trainer.save(RUN / "warmup.pt")
    display(FileLink(str((RUN / "warmup.pt").relative_to(Path.cwd()))))
    """,
    )
    add(
        "markdown",
        """
    ## 5. Train — every checkpoint gets a download link
    `step-XXXXXXXX.pt` contains the model, optimizer, tokenizer, sampling state,
    dataset identity and progress. `best.pt` is selected only on development loss;
    `latest.pt` is saved when this call finishes or is interrupted normally.
    Provider termination can preserve only files already written to pooled storage.
    Download any numbered checkpoint while training continues using its link or
    AIKosh's file-copying window. A stopped kernel is not required to copy a completed
    checkpoint; writes are atomic. Resume by setting `RESUME` above and rerunning.
    """,
    )
    add(
        "code",
        """
    def checkpoint_ready(path, event):
        print(json.dumps(event))
        display(FileLink(str(path.relative_to(Path.cwd()))))
    trainer.fit(train_data, dev_data, RUN, steps=STEPS,
                checkpoint_every=CHECKPOINT_EVERY, dev_windows=DEV_WINDOWS,
                callback=checkpoint_ready)
    display(FileLink(str((RUN / "latest.pt").relative_to(Path.cwd()))))
    display(FileLink(str((RUN / "best.pt").relative_to(Path.cwd()))))
    """,
    )
    add(
        "markdown",
        """
    ## 6. Test a checkpoint by conversing
    `TEST_CHECKPOINT` can point to any downloaded/uploaded training checkpoint.
    The training object is released first to avoid two models plus two optimizers
    on your 5 GB GPU. Responses are generated by the circuit, without an external
    language model. They may be wrong or incoherent. State persists across calls;
    use `brain.reset()` for a fresh conversation. No testing answer becomes a label.
    """,
    )
    add(
        "code",
        """
    TEST_CHECKPOINT = RUN / "best.pt"
    del trainer
    if "model" in globals():
        del model
    import gc
    gc.collect()
    if DEVICE == "cuda":
        torch.cuda.empty_cache()
    trainer = ConversationTrainer.load(TEST_CHECKPOINT, device=DEVICE)
    brain = trainer.session(continuous=True)
    def chat(question):
        result = brain.ask(question, max_tokens=8 if SMOKE else 64)
        print(result["text"] or "[No text generated]")
        return result
    chat("Hello. What can you explain?")
    # chat("What is 12 plus 7?")
    # chat("Why?")
    # brain.reset()
    INTERACTIVE_CHAT = False
    if INTERACTIVE_CHAT:
        while True:
            question = input("You (/quit, /reset): ").strip()
            if question == "/quit": break
            if question == "/reset": brain.reset()
            elif question: chat(question)
    """,
    )
    add(
        "markdown",
        """
    ## 7. Optional continuous activity, thoughts and replay
    `LiveBrain` advances the simulation independently of questions, with seeded
    stochastic background drive and activity regulation. It monitors internal
    state, alternates exploration/rest phases, and periodically generates separate
    candidate thoughts or dreams. Candidate branches are evaluated sequentially
    to honor the memory budget. This is **not evidence of subjective awareness**.
    All perceived, recalled and generated material stays distinguished.

    Optional autonomous rehearsal uses training windows and development retention
    checks. An update that worsens the chosen retention check is rolled back.
    Generated thoughts never become factual training targets. Do not train through
    another notebook cell while the live loop is running. Stop it before export,
    checkpoint loading or ordinary training. The model runs only while the notebook
    kernel (or your local runtime process) remains active and the computer is awake.
    """,
    )
    add(
        "code",
        """
    START_LIVE = False
    ENABLE_REHEARSAL = False
    life = LiveBrain(brain, trainer=trainer if ENABLE_REHEARSAL else None,
                     replay_windows=[train_data[i] for i in range(min(4, len(train_data)))],
                     validation_windows=[dev_data[0]])
    if START_LIVE:
        print(life.start())
    # life.ask("What should we investigate?")
    # life.think(possibilities=3)
    # life.dream()
    # life.status()
    # life.stop()
    """,
    )
    add(
        "markdown",
        """
    ## 8. Optional neuron-growth experiment
    Growth is bounded to 10% per decision, the checkpoint's synapse budget, and
    the local runtime's output-parameter cap. New neurons receive weak synapses;
    old learned connections are retained and indices are remapped. A candidate
    rehearses observed training windows, then must pass a separate retention check.
    Failed growth is discarded. Optimizer moments restart for the changed dimensions.
    This demonstrates a structural mechanism, not guaranteed improvement. Leave
    disabled during the initial 5 GB memory check; an allocation failure must not
    be interpreted as successful growth. Never use test windows for this gate.
    """,
    )
    add(
        "code",
        """
    ENABLE_GROWTH = False
    if ENABLE_GROWTH:
        life.stop()
        from cortex_ml.circuit.growth import grow_checked
        trainer, growth_report = grow_checked(trainer, [train_data[0]], [dev_data[0]], count=32, updates=2)
        print(growth_report)
        trainer.save(RUN / "after-growth.pt")
        brain = trainer.session()
        life = LiveBrain(brain)
        display(FileLink(str((RUN / "after-growth.pt").relative_to(Path.cwd()))))
    """,
    )
    add(
        "markdown",
        """
    ## 9. Held-out evaluation, then export for local use
    The test set is separate. These teacher-forced token metrics are not a claim
    of conversational understanding; inspect free conversations above as well.
    The full split can be evaluated by increasing `TEST_WINDOWS`. Repeated tuning
    against it requires a new untouched test set for honest final evaluation.

    `local-brain.json` includes the complete trained circuit and tokenizer and can
    be loaded by the rebuilt Rust CLI. JSON is larger than the PyTorch checkpoint
    (roughly 700 MB for 30M weights); host RAM is separate from the GPU budget.
    The small supplied arithmetic-operation head is not trained by this conversation
    corpus. Use neural conversation, not its untrained operation selector.
    """,
    )
    add(
        "code",
        """
    life.stop()
    TEST_WINDOWS = 1 if SMOKE else 32
    test_data = TokenDataset(DATA, "test", window=WINDOW_SYMBOLS)
    evaluation = trainer.evaluate(test_data, max_windows=TEST_WINDOWS)
    (RUN / "evaluation.json").write_text(json.dumps(evaluation, indent=2))
    print(evaluation)
    exported = trainer.export_rust(RUN / "local-brain.json")
    checksum = hashlib.sha256(exported.read_bytes()).hexdigest()
    (RUN / "local-brain.sha256").write_text(checksum + "  local-brain.json\\n")
    for path in (exported, RUN / "local-brain.sha256", RUN / "evaluation.json"):
        display(FileLink(str(path.relative_to(Path.cwd()))))
    """,
    )
    add(
        "markdown",
        """
    ## Local commands
    Copy `local-brain.json` to a new local folder. From your Cortex checkout:

    ```sh
    cargo build --release -p cortex-brain --bin cortex-brain-circuit
    target/release/cortex-brain-circuit stream /path/to/local-brain.json
    ```

    `stream` keeps neural state across turns; `/reset` clears activity and `/quit`
    exits. For continuous activity between turns, use the Python runtime:

    ```sh
    PYTHONPATH=ml ml/.venv/bin/python -m cortex_ml.circuit.life /path/to/local-brain.json
    ```

    Its `/status`, `/think`, `/dream` and `/quit` commands expose the ongoing loop.
    `--headless` keeps it running without an input prompt; stop with Ctrl-C or your
    service manager. It does not automatically replace existing Cortex services.
    For portable checkpoint training/resume use the notebook's numbered `.pt` files.

    The circuit remains an approximation: designed region groupings, simplified
    dendrites and spikes, engineered sensory encoding and surrogate-gradient training.
    No complete brain replica, biological sleep, subjective experience, autonomous
    scientific discovery, or general speech/3D understanding is established.
    """,
    )
    notebook = {
        "nbformat": 4,
        "nbformat_minor": 5,
        "metadata": {
            "kernelspec": {
                "name": "python3",
                "language": "python",
                "display_name": "Python 3 (ipykernel)",
            },
            "language_info": {"name": "python", "version": "3.11"},
        },
        "cells": cells,
    }
    notebook_path = output / "Cortex_AIKosh_30M.ipynb"
    notebook_path.write_text(json.dumps(notebook, indent=1))
    data_zip = output / "conversation_data.zip"
    with zipfile.ZipFile(data_zip, "w", zipfile.ZIP_DEFLATED) as bundle:
        for p in sorted(data.rglob("*")):
            if p.is_file():
                bundle.write(p, Path("conversation_data") / p.relative_to(data))
    instructions = output / "START_HERE.txt"
    instructions.write_text(
        "Upload Cortex_AIKosh_30M.ipynb and conversation_data.zip into the same AIKosh folder.\n"
        "Use a Python 3.11+ CUDA PyTorch kernel with numpy and tokenizers. Run cells in order.\n"
        "The notebook enforces a 4.25 GB PyTorch allocator cap and reports the measured peak; also check the provider's full GPU-memory monitor.\n"
        "Every completed numbered .pt checkpoint gets a download link; resume with RESUME.\n"
        "Your source conversation data only: Dolly, OASST1, OASST2. No authored examples were added.\n"
        "The 30M+ count is mostly output synapses; it does not prove brain equivalence or awareness.\n"
    )
    archive = output / "Cortex_AIKosh_30M_Upload.zip"
    guide = output / "ARCHITECTURE_AND_LIMITS.md"
    shutil.copy2(ROOT / "docs/AIKOSH_TRAINING.md", guide)
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_STORED) as bundle:
        for p in (notebook_path, data_zip, instructions, guide):
            bundle.write(p, p.name)
    shutil.copy2(data / "manifest.json", output / "dataset_manifest.json")
    print(
        json.dumps(
            {
                "notebook": str(notebook_path),
                "dataset_zip": str(data_zip),
                "upload_bundle": str(archive),
                "bytes": archive.stat().st_size,
            },
            indent=2,
        )
    )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--data", type=Path, default=ROOT / "target/aikosh-conversation-data"
    )
    parser.add_argument("--output", type=Path, default=ROOT / "artifacts/aikosh")
    args = parser.parse_args()
    build(args.data, args.output)
