# Cortex code embeddings

This directory contains the optional local embedding service used by semantic
code search in the coding harness.

```sh
uv sync --project ml
uv run --project ml python -m cortex_ml.embedding.server --port 8765
```

The service listens on loopback, exposes `GET /health` and `POST /embed`, and caches
embeddings to avoid repeated work. The default model is `google/embeddinggemma-2`.
Set `CORTEX_EMBEDDING_MODEL` to use a local model directory. Access to the default
model may require accepting its license and authenticating with Hugging Face.

Regular chats, file tools, lexical search, indexing, and verification work without
this service. No training or neural memory components are included.
