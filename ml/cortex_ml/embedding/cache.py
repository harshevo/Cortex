"""Remembering vectors so the same code is never embedded twice.

An agent run indexes a project, edits a file or two, and searches again.
Almost every chunk is unchanged between those searches, and between runs on
the same project. Looking vectors up by a hash of their text makes repeat
indexing nearly free.
"""

import hashlib
from collections import OrderedDict

import numpy as np

from cortex_ml.embedding.model import Document, Embedder


def document_key(document: Document) -> str:
    digest = hashlib.sha256()

    digest.update(document.title.encode("utf-8"))
    digest.update(b"\x00")
    digest.update(document.text.encode("utf-8"))

    return digest.hexdigest()


class CachedEmbedder:
    """Wraps an embedder with a bounded least-recently-used cache."""

    def __init__(self, embedder: Embedder, max_entries: int = 200_000):
        self.embedder = embedder
        self.max_entries = max_entries
        self.vectors: OrderedDict[str, np.ndarray] = OrderedDict()

        self.hits = 0
        self.misses = 0

    @property
    def dimension(self) -> int:
        return self.embedder.dimension

    def embed_queries(self, queries: list[str]) -> np.ndarray:
        # Queries are short and rarely repeat, so they are not cached.
        return self.embedder.embed_queries(queries)

    def embed_documents(self, documents: list[Document]) -> np.ndarray:
        keys = [document_key(document) for document in documents]

        missing = {
            key: document
            for key, document in zip(keys, documents)
            if key not in self.vectors
        }

        self.hits += len(documents) - len(missing)
        self.misses += len(missing)

        if missing:
            computed = self.embedder.embed_documents(list(missing.values()))

            for key, vector in zip(missing.keys(), computed):
                self._store(key, vector)

        if not keys:
            return np.zeros((0, self.dimension), dtype=np.float32)

        return np.stack([self._load(key) for key in keys])

    def _store(self, key: str, vector: np.ndarray) -> None:
        self.vectors[key] = vector

        while len(self.vectors) > self.max_entries:
            self.vectors.popitem(last=False)

    def _load(self, key: str) -> np.ndarray:
        self.vectors.move_to_end(key)

        return self.vectors[key]
