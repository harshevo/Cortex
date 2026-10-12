"""EmbeddingGemma 2, loaded for code search.

The model maps a search query and a piece of code into the same vector
space, so that related ones end up close together. Only its text part is
loaded (about 270M parameters); the vision and audio encoders are left out.
"""

from dataclasses import dataclass

import numpy as np
import torch
from sentence_transformers import SentenceTransformer

MODEL_NAME = "google/embeddinggemma-2"

# The model is trained with a short instruction in front of every input.
# These are the two it defines for code search.
QUERY_PROMPT = "CodeRetrieval"
DOCUMENT_TEMPLATE = "title: {title} | text: {text}"

# Text-only configuration, as described in the model card.
TEXT_ONLY = {"vision_config": None, "audio_config": None}


@dataclass(frozen=True)
class Document:
    """A piece of code to index. The title is usually its path and name."""

    title: str
    text: str


def pick_device() -> str:
    if torch.backends.mps.is_available():
        return "mps"

    if torch.cuda.is_available():
        return "cuda"

    return "cpu"


class Embedder:

    def __init__(self, device: str | None = None, batch_size: int = 16):
        self.device = device or pick_device()
        self.batch_size = batch_size

        # float16 makes this model return NaN, so stay on float32.
        self.model = SentenceTransformer(
            MODEL_NAME,
            device=self.device,
            config_kwargs=TEXT_ONLY,
            model_kwargs={"torch_dtype": torch.float32},
        )

        self.dimension = int(self.model.encode("probe").shape[-1])

    def embed_queries(self, queries: list[str]) -> np.ndarray:
        """Return one unit-length vector per query."""

        return self._encode(queries, prompt_name=QUERY_PROMPT)

    def embed_documents(self, documents: list[Document]) -> np.ndarray:
        """Return one unit-length vector per document."""

        texts = [
            DOCUMENT_TEMPLATE.format(title=document.title or "none", text=document.text)
            for document in documents
        ]

        return self._encode(texts, prompt_name=None)

    def _encode(self, texts: list[str], prompt_name: str | None) -> np.ndarray:
        if not texts:
            return np.zeros((0, self.dimension), dtype=np.float32)

        vectors = self.model.encode(
            texts,
            prompt_name=prompt_name,
            batch_size=self.batch_size,
            normalize_embeddings=True,
            convert_to_numpy=True,
            show_progress_bar=False,
        )

        return vectors.astype(np.float32)
