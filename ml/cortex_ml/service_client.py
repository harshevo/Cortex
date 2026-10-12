"""Calling the embedding service from other Python code.

Training scripts use the running service instead of loading a second copy
of the embedding model, and share its cache.
"""

import json
import urllib.request

import numpy as np

BATCH_SIZE = 32


def _post(url: str, body: dict) -> np.ndarray:
    request = urllib.request.Request(
        f"{url.rstrip('/')}/embed",
        data=json.dumps(body).encode("utf-8"),
        headers={"content-type": "application/json"},
    )

    with urllib.request.urlopen(request, timeout=600) as response:
        vectors = json.loads(response.read())["vectors"]

    return np.array(vectors, dtype=np.float32)


def embed_queries(url: str, texts: list[str]) -> np.ndarray:
    batches = [
        _post(url, {"kind": "query", "texts": texts[start : start + BATCH_SIZE]})
        for start in range(0, len(texts), BATCH_SIZE)
    ]

    return np.concatenate(batches)


def embed_documents(url: str, documents: list[tuple[str, str]]) -> np.ndarray:
    """`documents` are `(title, text)` pairs."""

    batches = []

    for start in range(0, len(documents), BATCH_SIZE):
        items = [
            {"title": title, "text": text}
            for title, text in documents[start : start + BATCH_SIZE]
        ]

        batches.append(_post(url, {"kind": "document", "documents": items}))

    return np.concatenate(batches)
