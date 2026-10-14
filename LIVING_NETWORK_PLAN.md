# Persistent learning network

The network must have a lifetime independent of a conversation or desktop window. One local background service owns the persistent network; clients deliver user text, assistant text, tool events and feedback. The service continues replay and maintenance while clients are absent, resumes from a checkpoint, and can start at login.

## First implementation

- A single writer with a private local socket, durable event inbox and experience archive, atomic checkpoints, and a cached snapshot for the UI.
- A bounded online language learner with 64 spiking cells, nonlinear dendritic compartments, plastic input synapses and an associative decoder for vocabulary, word context, next-word continuations and explicitly sourced categorical relations. User corrections change the weight of the relevant assistant example within its chat worker. Observed assertions retain their provenance; repetition is not verification. The current spiking population is fixed; displayed word neurons grow separately. Recurrent spike connections are inhibitory/excitatory and fixed; dendritic input synapses adapt.
- Background replay of actual experience with a separate replay counter. Generated continuations never train the learner. Accuracy is measured before learning on fresh examples, separately from replay.
- The inspector exposes service activity, vocabulary, observed tokens, language prediction accuracy, replay, continuations and conditional relation chains.
- Existing dendritic action predictions, imagination and planning remain available.

## Subsequent research gates

The initial implementation passed 39 living-network tests, the coding-agent brain/memory integration test, strict Clippy checks for the learning service, and release builds. The installed macOS service retained its learned parameters through a forced process restart with no desktop running. The installed desktop displayed service telemetry and the spiking/dendritic learner. An initial import of saved Cortex dialogue produced 33 observed messages and 542 vocabulary entries. These are mechanism and integration checks, not evidence of general intelligence; the initial next-word record was 4.8% against a 4.6% common-word baseline and is not a held-out generalization result.

1. Compare SNN, dendritic and a combined spiking/dendritic learner on the same held-out sequence and retention tasks. The current SNN reservoir trains a readout; its recurrent synapses are not plastic.
2. Learn reusable semantic representations and action-conditioned transitions. Evaluate paraphrases, changed tasks, delayed feedback and retained skills against fixed and memory-only baselines.
3. Train a language decoder on an appropriate curriculum. Small chat histories and a word-transition learner do not establish LLM-like language or reasoning.
4. Evaluate planning and causal interventions in an environment with verifiable outcomes. Add measured uncertainty and a computational self-model.

Continuous operation, neuron counts, spikes, replay and self-descriptions do not demonstrate consciousness or guarantee general intelligence. A sleeping or powered-off computer cannot execute the service. Useful progress requires new evidence or a consolidation task; when neither exists the service remains available without manufacturing training data.

## Continuous operation (added 2026-10-14)

When there is no new experience and replay is exhausted, the service dreams every ten seconds: it rehearses one taught memory or stored episode, grows spiking cells as the vocabulary grows (64 + vocabulary/16, at most 512), replaces cells silent for 20,000 presentations, regrows synapses stuck at the weight floor, fades unused word synapses and associations, and removes word neurons left with no connections. It keeps a curiosity list (questions it could not answer, memories it keeps getting wrong, frequent words never taught) that the LLM teacher is asked to answer first. Rehearsal reuses real memories only and is never scored as evidence. Chat, code and trading sessions all feed the same service.
