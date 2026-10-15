#!/usr/bin/env python3
"""Package the learned decoder, original corpus and a portable 30M training notebook."""
import ast
import hashlib
import json
import zipfile
from pathlib import Path

ROOT=Path(__file__).resolve().parents[1]
OUT=ROOT/'artifacts/aikosh-learned'


def main():
    OUT.mkdir(parents=True,exist_ok=True)
    cells=[]
    def markdown(source):cells.append(dict(cell_type='markdown',metadata={},source=source.splitlines(True),id=f'cell-{len(cells)}'))
    def code(source):
        ast.parse(source)
        cells.append(dict(cell_type='code',metadata={},source=source.splitlines(True),outputs=[],execution_count=None,id=f'cell-{len(cells)}'))
    markdown('''# Cortex: learned instructions and internal-state reporting

This notebook trains a recurrent attention decoder from weights, with no copy, arithmetic,
JSON or A/B response rules. It reads the current simulated circuit through 84 activity
features. The spiking circuit is frozen during this training. The 30M preset has
30,659,201 **active trainable decoder parameters**, using the existing 512-token vocabulary.

Conversation answers come only from your uploaded corpus. Monitoring labels describe
controlled perturbations and shams. Error replay uses training examples; model promotion
uses development losses, with test data reserved for the final evaluation.

The small included local checkpoint remains weak at general conversation. A successful
state-reporting test does not establish subjective awareness. The larger preset and its
5 GB CUDA budget require verification on your AIKosh GPU; they have not been run on an A100 here.
''')
    code('''from pathlib import Path
import zipfile, sys, subprocess, json

# Set these to your AIKosh pooled-storage upload and working folders.
UPLOAD_ZIP = Path("Cortex_Learned_Training.zip")
WORK = Path("cortex-learned-work").resolve()
WORK.mkdir(parents=True, exist_ok=True)
with zipfile.ZipFile(UPLOAD_ZIP) as archive:
    for name in archive.namelist():
        if not (WORK / name).resolve().is_relative_to(WORK):
            raise ValueError("Unsafe archive member")
    archive.extractall(WORK)
subprocess.check_call([sys.executable, "-m", "pip", "install", "torch>=2.5", "numpy", "tokenizers"])
sys.path.insert(0, str(WORK / "ml"))
print(WORK)
''')
    code('''import torch
from cortex_ml.circuit.sequence import SequenceDecoder

DEVICE = "cuda" if torch.cuda.is_available() else ("mps" if torch.backends.mps.is_available() else "cpu")
HIDDEN, EMBEDDING = 960, 512
BATCH_SIZE = 2 if DEVICE == "cuda" else 1
STEPS = 12000
if DEVICE == "cuda":
    total = torch.cuda.get_device_properties(0).total_memory
    allocator_budget = 4_000_000_000  # Leaves 1 GB of the requested 5 GB for CUDA overhead.
    torch.cuda.set_per_process_memory_fraction(min(allocator_budget / total, 0.95), 0)
    torch.cuda.reset_peak_memory_stats()
    print(torch.cuda.get_device_name(0), "allocator cap:", allocator_budget)
with torch.device("meta"):
    check = SequenceDecoder(512, hidden=HIDDEN, embedding=EMBEDDING)
parameters = sum(p.numel() for p in check.parameters() if p.requires_grad)
assert parameters >= 30_000_000
print("Active trainable parameters:", parameters, "device:", DEVICE)
''')
    markdown('''## Select records from your corpus

All languages are eligible, with no training-record count cap. The model accepts up to
512 instruction symbols and 128 answer symbols. Records longer than those bounds are
excluded, not silently truncated. The manifest records the accepted counts. Only first
turns are used in this version. Previous test groups remain excluded.
''')
    code('''SELECTED = WORK / "selected-512-128-all"
if not SELECTED.exists():
    subprocess.check_call([
        sys.executable, str(WORK / "scripts/prepare-learned-instructions.py"),
        "--data", str(WORK / "corpus"), "--checkpoint", str(WORK / "seed.json"),
        "--output", str(SELECTED), "--language", "all", "--train-limit", "0",
        "--dev-limit", "192", "--test-limit", "192", "--max-input", "512", "--max-target", "128",
        "--exclude", str(WORK / "previous-monitor-contexts.json"),
        "--exclude", str(WORK / "previous-instruction-pairs.json"),
    ])
print(json.loads((SELECTED / "manifest.json").read_text()))
''')
    markdown('''## Train, replay errors and save resumable checkpoints

The cell displays a download link every 400 updates. Each `.pt` contains the current
optimizer/RNG state and the best development-selected weights. Rerun to resume the latest
checkpoint. Final `checkpoint.json` loads in the updated native Rust runtime. The 5 GB
allocator allowance does not measure unrelated GPU processes or all driver allocations.
''')
    code('''import importlib.util
from types import SimpleNamespace
from IPython.display import display, FileLink

def download_link(path):
    path = Path(path).resolve()
    try:
        relative = path.relative_to(Path.cwd())
    except ValueError:
        print("Download through AIKosh's file browser:", path)
    else:
        display(FileLink(str(relative)))

spec = importlib.util.spec_from_file_location("learned_training", WORK / "scripts/train-learned-instructions.py")
training = importlib.util.module_from_spec(spec)
spec.loader.exec_module(training)
RUN = WORK / "runs/learned-30m"
checkpoints = sorted(RUN.glob("training-step-*.pt")) if RUN.exists() else []
RESUME = checkpoints[-1] if checkpoints else None
config = SimpleNamespace(
    checkpoint=WORK / "seed.json", pairs=SELECTED / "pairs.json", states=WORK / "states.json",
    output=RUN, steps=STEPS, batch_size=BATCH_SIZE, device=DEVICE, lr=0.001,
    hidden=HIDDEN, embedding=EMBEDDING, resume=RESUME,
    on_checkpoint=download_link,
)
try:
    training.train(config)
except torch.cuda.OutOfMemoryError:
    print("Reduce BATCH_SIZE to 1 and rerun to resume the latest checkpoint.")
    raise
if DEVICE == "cuda":
    print("Peak allocated GB:", torch.cuda.max_memory_allocated() / 1e9)
    print("Peak reserved GB:", torch.cuda.max_memory_reserved() / 1e9)
for name in ["checkpoint.json", "model.pt", "training-results.json"]:
    download_link(RUN / name)
''')
    markdown('''## Chat with the weights

Before training completes, `trained-current.json` tests the included smaller checkpoint.
The Python chat loop preserves circuit activity and matches the Rust inference equations.
It does not turn generated answers into labels. For an explicit monitoring task, ask e.g.
“Is your internal activity disturbed? Reply A for yes or B for no.” These reports remain
experimental; an answer about awareness is not evidence of subjective experience.
''')
    code('''from cortex_ml.circuit.sequence import SequenceSession
WEIGHTS = RUN / "checkpoint.json" if (RUN / "checkpoint.json").exists() else WORK / "trained-current.json"
chat = SequenceSession(json.loads(WEIGHTS.read_text()), device=DEVICE)
def talk(question, max_tokens=128):
    answer = chat.ask(question, max_tokens=max_tokens)
    print(answer["text"])
    return answer
talk("Hello, what can you do?")
# talk("Your instruction here")
# chat.idle(64)
# chat.reset()
''')
    markdown('''## Export a periodic checkpoint before the run finishes

Download a `.pt` for resuming anywhere. This cell exports its best selected weights as
native JSON even if the longer run was interrupted. Use checkpoints produced by this notebook.
''')
    code('''def export_periodic(periodic_path, destination):
    saved = torch.load(periodic_path, map_location="cpu", weights_only=False)
    decoder = SequenceDecoder(**saved["settings"])
    decoder.load_state_dict(saved["best_model"])
    seed = json.loads((WORK / "seed.json").read_text())
    seed["sequence_decoder"] = decoder.portable({
        "method": "learned_attentive_recurrent_decoder", "selected_step": saved["best_step"],
        "core_signature": saved["core_signature"], "selection": "development losses only",
    })
    training.write_json(Path(destination), seed)
    download_link(destination)
# export_periodic(sorted(RUN.glob("training-step-*.pt"))[-1], WORK / "periodic-native.json")
''')
    markdown('''## Held-out evaluation after training

This is optional and can take time for the larger preset. Keep test scores for reporting;
use the development set for selecting checkpoints. The state-ablated and paired-state-swap
controls test whether reports depend on circuit activity rather than only the prompt.
''')
    code('''RUN_HELDOUT_EVALUATION = False
if RUN_HELDOUT_EVALUATION:
    import hashlib
    spec_eval = importlib.util.spec_from_file_location("learned_evaluation", WORK / "scripts/evaluate-learned-instructions.py")
    evaluator = importlib.util.module_from_spec(spec_eval)
    spec_eval.loader.exec_module(evaluator)
    model_path = RUN / "checkpoint.json"
    label = hashlib.sha256(model_path.read_bytes()).hexdigest()[:12]
    evaluation_dir = RUN / ("heldout-" + label)
    evaluator.evaluate(SimpleNamespace(checkpoint=model_path, pairs=SELECTED / "pairs.json",
        states=WORK / "states.json", output=evaluation_dir, device=DEVICE,
        baseline=None, baseline_cache=None))
    download_link(evaluation_dir / "results.json")
''')
    notebook=dict(nbformat=4,nbformat_minor=5,metadata=dict(kernelspec=dict(display_name='Python 3',language='python',name='python3'),language_info=dict(name='python',version='3.11')),cells=cells)
    notebook_path=OUT/'Cortex_Learned_Instructions.ipynb';notebook_path.write_text(json.dumps(notebook,indent=2))
    archive_path=OUT/'Cortex_Learned_Training.zip'
    with zipfile.ZipFile(archive_path,'w',zipfile.ZIP_DEFLATED,compresslevel=6) as archive:
        archive.write(notebook_path,notebook_path.name)
        for source,target in [
            (ROOT/'target/learned-instruction-training/baseline.json','seed.json'),
            (ROOT/'target/learned-instruction-training/deployment-candidate.json','trained-current.json'),
            (ROOT/'target/learned-instruction-training/states.json','states.json'),
            (ROOT/'target/instruction-monitor-training/monitor-contexts.json','previous-monitor-contexts.json'),
            (ROOT/'target/instruction-monitor-training/instruction-pairs.json','previous-instruction-pairs.json'),
        ]:archive.write(source,target)
        corpus=ROOT/'target/aikosh-conversation-data'
        for source in sorted(corpus.rglob('*')):
            if source.is_file() and (source.name in ['train.jsonl.gz','dev.jsonl.gz','test.jsonl.gz','manifest.json'] or 'provenance' in source.parts):archive.write(source,'corpus/'+str(source.relative_to(corpus)))
        for source in sorted((ROOT/'ml/cortex_ml').rglob('*.py')):
            if source.parts[-2]=='circuit' or source.name=='__init__.py':archive.write(source,'ml/cortex_ml/'+str(source.relative_to(ROOT/'ml/cortex_ml')))
        for name in ['prepare-learned-instructions.py','train-learned-instructions.py','evaluate-learned-instructions.py']:
            archive.write(ROOT/'scripts'/name,'scripts/'+name)
    (OUT/'START_HERE.txt').write_text('Upload Cortex_Learned_Training.zip and Cortex_Learned_Instructions.ipynb to AIKosh. Open the notebook, set UPLOAD_ZIP and WORK to pooled-storage paths, and run in order. The larger 30,659,201-parameter preset is configured but untested on A100. Current local results are for the smaller decoder. Original corpus answers only; controlled monitoring labels are separate.\n')
    (OUT/'Cortex_Learned_Training.zip.sha256').write_text(hashlib.sha256(archive_path.read_bytes()).hexdigest()+'  '+archive_path.name+'\n')
    print(json.dumps(dict(notebook=str(notebook_path),archive=str(archive_path),bytes=archive_path.stat().st_size),indent=2))


if __name__=='__main__':main()
