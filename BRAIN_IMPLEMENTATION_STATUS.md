# Brain implementation, 15 October 2026

## Delivery scope

Extend the existing brain without replacing the sequence learner or its saved
weights. Add grounded arithmetic, learned declarative knowledge and formulae,
bounded inference, English conversation with follow-up context, and inspectable
evidence. Built-in algorithms must be distinguished from learned associations.
Unknown questions must not be presented as verified answers. Experience means
recorded observations; awareness means internal monitoring, not demonstrated
subjective experience. Universal learning remains an open research objective.

The experimental spiking circuit follows `BRAIN_CIRCUIT_PLAN.md`: deterministic
CPU dynamics, sparse seeded wiring, explicit resource caps, and isolated tests
before integration. The hybrid language system above is separate from the new
circuit's experimental token decoder, described in the final section.

## Original work list

- Fix mathematical token loss and incomplete reasoning-goal matching.
- Add bounded arithmetic execution whose explanation is produced by execution.
- Learn factual statements, question/answer pairs, and explicit formula rules,
  retaining source memories and contradictions.
- Add conversational context, uncertainty, correction, and inspectable traces.
- Connect the conversation and inspector to CLI, service, and desktop views.
- Implement and evaluate the initial isolated neural circuit milestone.
- Test generalization, persistence, conflicting lessons, numerical errors, and
  integration. Record actual results and remaining limitations here.

## Implemented and measured

- Exact bounded rational arithmetic, decimal preservation, operator precedence,
  signed quantities, equality checking, and explanations emitted by execution.
- Session-specific object collections with counting/removal events, bounded
  evidence history, hypothetical scenarios, and follow-up explanations.
- Source-linked declarative premises, explicit category inference, dialogue
  indexing, formula learning, SI dimension checks, inverse formula solving,
  conflicting-evidence responses, and human corrections.
- Bounded adaptive rehearsal with a separate retention criterion. A read-only
  inference path borrows wiring instead of copying the checkpoint. Equivalence
  tests compare it with the original engine, including restored weights.
- Curiosity retains unanswered questions, revisits them while idle, resolves
  them from lessons, and reduces priority for familiar unpredictable noise.
- Dreams favor supported continuations, limit repetitions and scene changes,
  and never enter the factual store or fresh-accuracy counters as observations.
- Brain inspector conversation, introductory-curriculum control, reasoning
  evidence, learning measurements, source memories, and process descriptions.
- Separate terminal/desktop conversation contexts; service restart tests.
- Version 2 atomic checkpoints include cortex, mind, conversation and budget.
  Legacy split checkpoints load; the JSON sidecar is now an inspectable export.

Local experiment data was found under
`/Users/harshevo/ai_projects/research_training_data/datasets`; the requested
`research_datasets/data` directory was not found in the project locations checked.
The bounded sample contains 32 training examples and 64 evaluation cases.
GSM8K uses its official train/test separation, Dolly uses a deterministic hash
split, and SciBench is evaluation only. Expected test answers never reach the
brain. Solution equations are supplied inputs in a separately labelled subtest.

Across ten small synthetic sequence trials, frozen prediction accuracy was
15.0% after one exposure, 82.9% after adaptive study, and 82.1% after four fixed
passes. Adaptive study used 69 passes against 120 fixed passes. These trials use
new concatenations of familiar sequences, not broad English comprehension.
Additional replay did not improve this probe. Study also measures retention,
so fewer passes do not by themselves establish a wall-time advantage; the JSON
report includes timings.

The sample achieved 16/16 on supplied-equation execution, 16/16 on recall of
taught maths answers, and 12/16 on taught conversation answers. The four
conversation failures were conflicting source responses to duplicate questions;
they were reported as conflicts. On unseen full questions the current engine
scored 0/16 for conversation, 0/16 for maths word problems, and 0/16 for science
word problems, abstaining on all of them. These failures remain visible.

Reproduce with:

```sh
python3 scripts/prepare-brain-research.py /absolute/path/to/datasets --train 16 --test 16
cargo run -p cortex-brain --example learning_bench -- target/brain-research
```

The full report and experiment checkpoint are in `target/brain-research/`.
A compact report is in `eval/results/brain-learning-2026-10-15.json`.

## Remaining research

This is a hybrid sequence/knowledge/skill system. Arithmetic and quantity
operations are implemented algorithms, not discovered neural concepts.
Language normalization and inventory parsing accept a restricted grammar.
Learned formulae are assumptions; checking execution and units cannot establish
that a physical model applies to an arbitrary situation. For example, the
introductory work rule assumes a constant force parallel to displacement.

General semantic comprehension, autonomous discovery of mathematical rules,
advanced physics, robust long-term generalization and subjective experience
remain unestablished. Internal monitoring has no validated scalar awareness
score. The current sequence learner is not a membrane-voltage simulation.

The new membrane/dendrite circuit now has its own experimental implementation
and lab, described below. The sequence baseline remains intact. The broader
A–G acceptance gates and proposed large-scale growth are not complete.

## Verification

The original hybrid baseline had 105 passing brain tests, including frozen inference
matching the original engine, numerical failure cases, source conflicts,
quantity events, hypothetical separation, persistence migration and actual
socket-service restarts. CLI, tools and evaluation tests also passed (195 tests
across those packages and the brain), and 120 desktop tests passed, including
rendering populated brain panels. Strict Clippy passes for brain, CLI and tools.
A broader strict desktop lint run remains blocked by 11 pre-existing trading
crate lints in `cortex-market`; that run is not claimed clean.

Teacher reply quizzes use the combined conversation system and expose the
answer method in their event details. Continuation quizzes retain the separate
neural-sequence path. Historical teacher totals mix earlier behavior with later
runs and must not be interpreted as a controlled neural-learning benchmark.

## Local service rollout

The release service was installed and verified at protocol/checkpoint version 2.
The existing 10 source-memory notes and prior activity history were retained.
The introductory curriculum was taught, and live socket tests passed for
greetings, arithmetic explanations, sequential object-counting, follow-up why
questions, force, kinetic energy and a dream with source memories. The LLM
teacher remains stopped. The rebuilt desktop executable is
`target/debug/cortex-gui`; the updated service already answers existing clients.

Backup of the prior executable, checkpoint, sidecar and service configuration:
`/Users/harshevo/.cortex/backups/brain-refinement-20261015-154503`.
The detailed local smoke results are in `target/brain-research/live-smoke.json`.


## First circuit-only learning lab, 15 October 2026 (historical baseline)

The user's selected direction is learning inside the new circuit, accepting
limited conversation. No pretrained model or external language service is used
by this lab. It runs separately from the existing background brain because
unseen-language generalization is still inadequate.

Implemented under `crates/cortex-brain/src/circuit/`:

- Adaptive leaky integrate-and-fire neurons, exponential current decay, reset,
  refractory periods, adaptation and filtered spike traces. Excitatory cells
  have four coarse dendritic compartments with finite plateaus and rearming.
- Deterministic sparse recurrent wiring, source-defined excitation/inhibition,
  1–5 ms delayed events, input validation, fan-in, event and memory limits.
  Checkpoints retain pending events and continue bit-for-bit after restoration.
- The language experiment uses 1,792 spiking neurons, 5,888 functional excitatory
  branches, 512 external spike channels, 100,352 fixed connections, and 27 rate
  output units with 48,384 plastic weights. The default simulation configuration
  retains the plan's 256 inputs; language uses 512 to reduce sensor collisions.
- Fixed word/operator spike sensors feed the circuit. Supervised output
  connections learn question-to-program mappings; reward-only output connections
  use REINFORCE with weight decay. Recurrent and dendritic connections stay
  fixed. No recurrent e-prop, TD value learner or biological morphology is claimed.
- Sparse wiring currently favors local sources 70% of the time and otherwise
  samples globally. Compartments are randomly assigned; wired inhibitory inputs
  currently target somata. The plan's detailed adjacent projections, dedicated
  branch roles, local inhibitory subnetworks and learning rules remain proposals.
- The output program catalog is inferred from training GSM8K worked solutions
  plus a supplied introductory curriculum. Arithmetic, dimensional conversion,
  operand binding, physics rules and English response templates are supplied
  algorithms. A learned program choice is neither a stored question answer nor
  discovery of arithmetic laws inside the neuron dynamics.
- Local English chat, executed calculation evidence and follow-up `why`, explicit
  corrections, bounded experience replay, uncertainty-driven questions to teach,
  and a complete wiring inspector. Unanswered questions are learning candidates;
  they do not become facts. Replay reuses labeled questions, not subjective dreams.

The dataset root actually used is
`/Users/harshevo/ai_projects/research_training_data/datasets`.
Training comprises 243 GSM8K examples with grounded programs, 2,130 supplied
math exercises, 540 supplied physics exercises, 26 dialogue/abstention examples
and 100 Dolly questions labeled unsupported. There are 303,900 supervised
updates across 100 epochs. General English answer text is not learned here.
SciBench is evaluation only. The preparation script keeps source checksums and
checks train/development/evaluation question identity separation.

The final held-out results use the same learned weights with per-program
acceptance thresholds selected on development examples:

| Evaluation | Answered | Numeric matches / correct supported dialogue | Total |
| --- | ---: | ---: | ---: |
| Supplied arithmetic paraphrases and new values | 32 | 26 | 48 |
| Supplied physics paraphrases and new values | 36 | 36 | 36 |
| Limited dialogue | 2 | 2 | 6 |
| Full upstream GSM8K test | 87 | 1 | 1,319 |
| SciBench | 43 | 1 | 692 |
| Held-out general English, intentionally unsupported | 0 | 0 | 100 |

SciBench's single match compares only the numeric reference and does not
establish a correct scientific derivation or unit. The selected simple GSM8K
subset overlaps the full upstream evaluation; do not add its denominator to the
full-set total. Even high readout scores can be wrong. These results fail the
requested broad unseen-English/math/physics goal.

The first acceptance rule produced 26/48 supplied arithmetic and 36/36 supplied
physics matches but made 218 wrong GSM8K answers. A circuit-state novelty gate
then blocked familiar arithmetic and did not improve generalization; it was
rejected. Class-mean similarity remains diagnostic. All three reports are kept
in the compact research JSON. The final holdout was reused while assessing
acceptance revisions; it is not a fresh untouched test for every revision.

On four known cue inputs, the 160-neuron reward learner achieves all four
choices across ten seeds, immediately and after a 200 ms blank. Frozen and
memory-erased controls achieve 25%; shuffled rewards achieve 25–27.5% on average.
The delay depends on a 300 ms filtered spike trace, not demonstrated persistent
recurrent working memory. Without regularization, reward reversal recovers at
most one choice in 6,000 interactions. Weight decay allows all four reversed
choices in every seed after 500–1,800 interactions, while losing the old mapping.
These are narrow learning probes. They do not complete gates D–G: delay curves,
matched non-spiking/dendrite baselines, transfer and equal-compute replay
comparisons remain missing. Passive timestep convergence is checked; active
network convergence is not established.

The language encoder resets membrane state for each 80 ms question presentation.
The lab does not continuously simulate thinking while idle, and its numeric
execution registers are supplied algorithms.

There is no validated scalar awareness score, evidence of subjective experience,
free-form language generation or general scientific understanding. Population
names indicate wiring groups, not established biological functions.

The final package checks pass 213 tests across brain, CLI, tools and evaluation;
18 circuit tests cover dynamics, limits, checkpoint continuation, reward learning,
unit handling, numerical transfer, acceptance failures, replay and HTTP requests.
Strict Clippy passes for brain, CLI and tools. The release circuit binary was
built and its installed checksum matched. Browser checks verified the `1 + 3`
response, full wiring toggle and individual dendritic/readout inspection.
Additional local API smoke tests cover teaching, replay and restart persistence.

The lab is available at `http://127.0.0.1:8778`. Its checkpoint is
`/Users/harshevo/.cortex/brain/circuit.json`; its executable is
`/Users/harshevo/.cortex/bin/cortex-brain-circuit`. It is a local process,
not a newly installed login service. Restart it with:

```sh
/Users/harshevo/.cortex/bin/cortex-brain-circuit serve /Users/harshevo/.cortex/brain/circuit.json 8778
```

Or converse in a terminal:

```sh
/Users/harshevo/.cortex/bin/cortex-brain-circuit chat /Users/harshevo/.cortex/brain/circuit.json
```

The existing brain checkpoint and service are separate. The detailed current
report is `docs/research/brain-circuit-2026-10-15.json`; the browser screenshot,
per-question runtime smoke and full training artifacts are under
`target/circuit-research/`.

## Shared recurrent and English training experiment

The next implementation trains one shared sparse circuit with two rate readouts:
operation selection and autoregressive English tokens. Surrogate BPTT updates
recurrent/input synaptic magnitudes, neuronal threshold scales and dendritic
branch gains as well as both heads. The topology, signs, delays and population
assignments stay fixed. It is not the plan's proposed e-prop estimator. Hard event
timers and resets are detached, and plateau activation uses a local surrogate.

The language path uses ordered spike symbols and a 512-token byte-pair vocabulary
fitted on local training text. No pretrained model, embedding service or LLM
teacher participates. The sensory codec, exact arithmetic, physics rules and
operation response templates remain supplied algorithms. English generation uses
the trained token readout; it does not retrieve a taught answer or use those
templates. It currently produces incoherent text. Each question resets transient
state; learned multi-turn conversation memory is not implemented.

The confirmed dataset root is
`/Users/harshevo/ai_projects/research_training_data/datasets`.
The bounded answer sample contains 845 training examples (526 unique prompts),
84 clean development examples and 74 test examples (50 unique prompts).
Training includes 462 conversation answers, 376 OpenStax physics paragraphs and
only seven worked mathematics answers. OpenStax paragraphs can share a title
prompt. The separate operation curriculum retains its 3,039 training rows.
The English test prompts do not appear in either training stream. An audit found
eight English development prompts previously seen as unsupported in operation
training; the continuation excludes them. Their answer texts were not taught.

The paired comparisons use identical initial weights, example order, rates and
update budgets for frozen-core and plastic-core candidates. One seed was tested.
The first stage spent 600 updates per candidate and retained step 500 using
development loss. The continuation resumes the plastic core and trains each
candidate for 300 updates, with smaller head/core rates and a recent spike trace
for language. Its frozen control therefore already contains a learned core.
Optimizer moments restart. The continuation selects the frozen candidate by
development operation cross-entropy plus 0.5 times answer-token NLL.

| Stage / candidate | Held-out program accuracy, 128 rows | English answer-token NLL | English token accuracy |
| --- | ---: | ---: | ---: |
| First stage, frozen core | 29.69% | 6.0203 | 2.71% |
| First stage, plastic core | 46.09% | 5.9655 | 1.95% |
| Continuation, frozen learned core | 35.94% | 5.3021 | 3.84% |
| Continuation, plastic core | 35.16% | 5.2922 | 3.77% |

These are raw operation choices, not verified numerical answers. After
development-only acceptance calibration, the selected continuation abstains on
all supplied held-out maths/physics cases and the broad 1,319-question GSM8K and
692-question SciBench sets. Its loss improved while useful answering regressed.
It has **not replaced the earlier lab**. All operation holdouts are reused from
the preceding experiment, rather than fresh after every project revision.

On 51 answer examples with question-length matched alternatives, swapping the
question raises selected-candidate NLL by only 0.0166. This weak effect and the
incoherent question-only generations do not establish semantic comprehension.
There is no measured increase in awareness, subjective experience or unrestricted
reasoning. The wiring changes are working mechanisms, rather than evidence that
the project now behaves like a complete brain.

For the continuation workload, MPS median joint-update time was 0.5145 seconds,
versus 0.8798 on the best tested CPU setting (four threads). CPU settings of one,
four and eight threads were compared; GPU operations were synchronized, fallback
was disabled, and loss/synaptic-gradient parity passed. Setup/transfer times are
recorded separately. Five measured samples follow two warm-ups. This is a local
workload result, not an ANE claim or a universal GPU speedup.

The plastic continuation changed 89,464 synaptic magnitudes, all 1,792 thresholds
and all 5,888 active excitatory branch gains; its frozen control changed none of
those core parameters. Sparse topology and source signs remained intact. Rust
and tensor activity features agree within 0.000003 on the deployed full circuit,
and generation tokens agree. A real server restart preserved the checkpoint,
learned parameters and generation. Buffered checkpoint I/O reduced a measured
load-and-probe call from 6.25 to 0.64 seconds.

Verification: 216 Rust package tests passed, with all 126 brain library tests
rerun after the final I/O fix; 11 new Python circuit tests and strict Rust/Python
lints passed. The current-source release binary was installed with matching
checksums. Browser checks exercised the token decoder, complete wiring toggle,
four dendrites, learned thresholds and both output-head inspectors. The ordinary
teaching/replay controls still update operation readouts only; recurrent learning
runs through the explicit offline trainer.

The candidate inspector is `http://127.0.0.1:8779`, using
`/Users/harshevo/.cortex/brain/circuit-trained.json`. It runs separately from
the earlier lab at port 8778 and the normal hybrid brain service. Restart with:

```sh
/Users/harshevo/.cortex/bin/cortex-brain-circuit-trained serve /Users/harshevo/.cortex/brain/circuit-trained.json 8779
```

Use **Neural English** or terminal `/english QUESTION` to inspect generation.
The full training protocol is `ml/cortex_ml/circuit/README.md`; the compact
comparison is `docs/research/brain-plastic-circuit-2026-10-15.json`, with checkpoints,
per-question reports, browser proof and logs under `target/circuit-plastic/`.


## Native live circuit connection

The circuit HTTP inspector has been removed. Start the live checkpoint with
`cortex-brain-circuit serve CHECKPOINT [SOCKET]` and use the Cortex desktop Brain
view for live spikes, membrane voltages, conversation, teaching and replay.
The default socket is `~/.cortex/brain/circuit.sock`; set
`CORTEX_CIRCUIT_SOCKET` consistently in both processes to override it. Earlier
HTTP addresses in the historical results above describe the previous transport.
