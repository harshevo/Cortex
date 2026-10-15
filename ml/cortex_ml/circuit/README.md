# Shared spiking circuit training

This experiment learns from local examples without a pretrained model, external
embeddings, or an LLM teacher. It is separate from Cortex's hybrid brain service
and the other ML services in this repository. Broad English conversation,
mathematical understanding and physical reasoning have not been established.

The same sparse, delayed ALIF circuit supplies two rate readouts: a small
operation catalog and a locally learned English token distribution. There are
no separate language, maths and physics copies of the circuit. English answers
are generated autoregressively; operation answers use supplied exact arithmetic,
operand binding, unit handling and response templates. A learned operation
choice does not establish discovery of arithmetic or physics laws.

## What learns

Approximate surrogate BPTT updates recurrent and sensory synaptic magnitudes,
neuronal threshold scales, dendritic branch gains, and both output heads.
Source signs, topology, delays, time constants and population assignments stay
fixed. Synaptic magnitudes are bounded to 0..15, threshold scales to 0.5..2 and
branch gains to 0.1..4. Inhibitory cells have no active dendritic compartments.

Forward spikes are hard threshold events. Their backward derivative is
`1 / (1 + 5 * abs(v - threshold))^2`. Hard resets, refractory and plateau timers
are detached; a local sigmoid surrogate carries the plateau-indicator gradient.
This estimator is approximate and is not the e-prop rule proposed in the plan.
It uses supervised answer/error signals, rather than a claimed biological
learning mechanism. See the [surrogate-gradient reference](https://arxiv.org/abs/1705.11146).

A deterministic byte-pair codec is fitted on training text only. This is supplied
sensory preprocessing, not a neural discovery of a writing system. Ordered
dual-rail spike symbols preserve token order. Operation input masks numbers so
the readout chooses an operation independently of the actual operands. Padding
is excluded from operation features and answer losses. The answer head learns
only next-token targets after the question; teacher forcing supplies previous
answer tokens during training and token evaluation. Free generation receives
only the question and its own generated prefix.

Operation features use a 300 ms spike trace. The optional language feature is a
10 ms trace plus 0.1 times that slower trace. This supplies recent-token evidence
without duplicating the neurons or changing their spike dynamics.

## Reproduce

Use the existing `ml/.venv` or an environment with PyTorch and pytest. This path
does not use the pretrained models required by unrelated modules in `ml/`.

```sh
cargo build --release -p cortex-brain --bin cortex-brain-circuit
python3 scripts/prepare-circuit-research.py /Users/harshevo/ai_projects/research_training_data/datasets
python3 scripts/prepare-plastic-circuit.py /Users/harshevo/ai_projects/research_training_data/datasets
target/release/cortex-brain-circuit init target/circuit-plastic/initial.json
PYTHONPATH=ml ml/.venv/bin/python -m cortex_ml.circuit.train \
  --initial target/circuit-plastic/initial.json \
  --data target/circuit-plastic/data --output target/circuit-plastic/experiment \
  --steps 300 --batch 4 --ticks 2
target/release/cortex-brain-circuit calibrate \
  target/circuit-plastic/data/operations-dev.jsonl target/circuit-plastic/experiment/plastic.json
PYTHONPATH=ml ml/.venv/bin/python -m cortex_ml.circuit.evaluate \
  --data target/circuit-plastic/data --checkpoint target/circuit-plastic/experiment/plastic.json \
  --output target/circuit-plastic/experiment/heldout.json --device mps
target/release/cortex-brain-circuit serve target/circuit-plastic/experiment/plastic.json
```

The trainer benchmarks synchronized complete joint forward/backward/Adam steps
on CPU with 1/4/8 threads and MPS when available. It records setup/transfer time,
p50/p95, actual placement and CPU/GPU loss/gradient parity, then chooses the
fastest passing backend. MPS CPU fallback is disabled before importing PyTorch.
For evaluation use `--device cpu` if MPS is unavailable. Timing samples are a
bounded workload benchmark, not a guaranteed speedup on every sequence length.

Frozen and plastic candidates begin from identical weights and receive the same
sampled examples, update budget and learning rates. The frozen candidate updates
only its output heads. The selected state minimizes operation cross-entropy plus
0.5 times answer-token NLL on development data, including the unchanged initial
state as a candidate. Optimizer moments restart on `--resume`; resumption is not
bit-for-bit continuation of Adam. Vocabulary and operation catalogs must match.
If resuming an already plastic checkpoint, the frozen control measures further
readout learning over that learned core, rather than a never-trained reservoir.

The measured refinement used a 600-step pilot with head rates 0.01 and core rate
0.001, followed by a 300-step continuation from its selected plastic state with
`--resume --fast-language-trace --operation-lr .003 --language-lr .001 --core-lr .0003`.
See `docs/research/brain-plastic-circuit-2026-10-15.json` for results and hashes.
The pilot selected step 500; counters were corrected to count retained updates,
with the discarded 600-step budget retained in metadata. One earlier pilot and
one pre-audit continuation were stopped; neither is reported as a completed run.

## Evaluation limits

The bounded answer dataset contains Dolly/OASST conversation, GSM8K worked
answers and OpenStax explanatory paragraphs. Questions/groups are separated by
source; English development/test prompts seen by operation training are removed,
and operation holdouts are excluded from English training. The codec sees a
larger training-only pool before complete pairs are filtered to 96 symbols.
This leaves only seven worked maths answers in the training subset; it cannot
support claims about general mathematical explanation. OpenStax title prompts
can have several explanatory paragraphs, so these are language examples rather
than a uniquely scored physics QA benchmark. Sources retain hashes and provenance
in the local collection; no corpus is republished with the code.

Operation holdouts from the preceding experiment are reused and are not fresh
after all project revisions. Final English metrics include answer-token NLL,
token accuracy, exact token-length matched question swaps and question-only
generation examples. Lower teacher-forced NLL alone is not conversation success.
There is no validated awareness score or evidence of subjective experience.

`/english QUESTION` in terminal chat and **Neural English** in the local lab use
the token head. Ordinary **Ask**, `/teach` and `/replay` retain operation selection
and output-only corrections. Recurrent learning runs in the explicit offline
trainer; the lab does not continuously learn recurrent weights while idle.
Each English question resets transient circuit state. Learned multi-turn dialogue
memory is not implemented; the operation path's `why` uses an explicit last-answer
record and execution evidence.
The wiring API and neuron inspector expose both heads and all simulated edges.

```sh
PYTHONPATH=ml ml/.venv/bin/python -m pytest ml/tests/test_circuit.py -q
cargo test -p cortex-brain circuit::
```
