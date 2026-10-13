# cortex-ml

The learned components of Cortex, in Python. The Rust agent talks to them over local HTTP, so each can be restarted or replaced on its own.

## Layout

```
cortex_ml/
  embedding/
    model.py     EmbeddingGemma 2, loaded text-only for code search
    cache.py     remembers vectors by content hash
    server.py    the HTTP service the agent calls
  hope/
    model.py     HOPE-style memory model: blocks updated at different speeds, plus a memory written during use
    train.py     trains it on a Cortex memory database
    server.py    the HTTP service the agent calls with `--with hope`
  predictor/
    model.py     JEPA-style predictor of a file's embedding after an edit
    data.py      examples from git history and from the agent's run logs
    train.py     trains it and compares it with "nothing changes"
  service_client.py   lets training scripts use the running embedding service
```

## Embedding service

Uses `google/embeddinggemma-2` (Apache-2.0). Only the 270M-parameter text part is loaded. The first start downloads about 1.5 GB to the Hugging Face cache.

```bash
uv sync
```

```bash
uv run python -m cortex_ml.embedding.server --port 8765
```

Then, from the `cortex` directory, either search directly:

```bash
cargo run -- search "where trades are grouped into calendar days" --dir path/to/project
```

or give the agent the `semantic_search` tool:

```bash
cargo run -- run "fix the bug" --with semantic
```

The service listens on `127.0.0.1` only.

## HOPE-style memory model

A small reimplementation of the two ideas in Google's Nested Learning paper (no code or weights were released, so this is not a reproduction): a chain of blocks that learn at different speeds, and a memory that is written while the model is in use. It re-scores the earlier tasks that similarity search has found for a new task.

Every block starts as the identity, so an untrained model ranks exactly like plain cosine similarity and is safe to switch on.

Train it on a memory database the agent has filled (`--with memory`). An earlier task counts as useful to a later one when the later task changed a file the earlier one had read or changed:

```bash
uv run python -m cortex_ml.hope.train --memory ../.cortex/memory.db --out hope.pt
```

```bash
uv run python -m cortex_ml.hope.server --port 8766 --weights hope.pt
```

The agent's own memory is still too small to train on, so the weights in `hope.pt` come from public collections of solved GitHub issues (SWE-bench's training split and SWE-rebench's filtered split), read the same way: an earlier issue is useful to a later one in the same repository when their fixes changed a file in common.

```bash
uv run python -m cortex_ml.hope.public --out data/hope-examples.npz data/SWE-bench/data/train-00000-of-00001.parquet data/SWE-rebench/data/filtered-00000-of-00001.parquet
uv run python -m cortex_ml.hope.pretrain --examples data/hope-examples.npz --out hope.pt
```

Status: trained on 13,007 ranking tasks. On 2,057 tasks from repositories kept out of training, the useful earlier task is ranked first 47.8% of the time against 42.9% for plain cosine (mean reciprocal rank 0.618 against 0.576), among the 32 nearest earlier tasks. This is measured on GitHub issues, not on the agent's own runs; whether it helps the agent finish tasks has not been measured.

## Edit predictor

Predicts the embedding a file will have after a described change, instead of generating the changed file. Trains on a repository's git history and on the agent's own `edit_file` calls:

```bash
uv run python -m cortex_ml.predictor.train --repo path/to/repo --trajectories ../eval/results
```

It is scored on held-out edits against the baseline "the file stays as it is", and saved only if it wins. Repeated attempts at one task are kept out of the held-out set.

Status: on the 41 edits in tradelyzer's git history it does not beat the baseline (distance 0.017 against 0.009 on 9 held-out edits). It is not used by the agent.

## Fruit fly connectome

`cortex --with brain` generates its wiring by default. To use the measured wiring of the adult fly instead, convert the connectivity table published with the whole-brain model of Shiu et al. (FlyWire, materialisation 783):

```bash
uv run python -m cortex_ml.connectome.convert path/to/2025_Connectivity_783.parquet fly.csv
```

Then pass `--connectome fly.csv`. The table is `data/2025_Connectivity_783.parquet` (101 MB) in `github.com/eonsystemspbc/fly-brain`; this project keeps it in `eval/sources/fly-brain/`, which is not committed. It converts to 138,639 neurons, 15,091,983 connected pairs and 54,492,922 synapses.

## Tests

```bash
uv run pytest
```
