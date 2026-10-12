"""A small local HTTP service that turns text into vectors.

The Rust agent calls it; the model stays loaded between calls.

    uv run python -m cortex_ml.embedding.server --port 8765

Endpoints:

    GET  /health   -> {"model": ..., "dimension": ..., "device": ...}
    POST /embed    <- {"kind": "query", "texts": [...]}
                   <- {"kind": "document", "documents": [{"title": ..., "text": ...}]}
                   -> {"vectors": [[...], ...]}

It listens on 127.0.0.1 only.
"""

import argparse
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from cortex_ml.embedding.cache import CachedEmbedder
from cortex_ml.embedding.model import MODEL_NAME, Document, Embedder

MAX_BODY_BYTES = 64 * 1024 * 1024


class BadRequest(Exception):
    pass


def embed(embedder: CachedEmbedder, request: dict) -> list[list[float]]:
    """Handle the body of one /embed request."""

    kind = request.get("kind")

    if kind == "query":
        texts = request.get("texts")

        if not isinstance(texts, list) or not all(isinstance(text, str) for text in texts):
            raise BadRequest("`texts` must be a list of strings")

        return embedder.embed_queries(texts).tolist()

    if kind == "document":
        items = request.get("documents")

        if not isinstance(items, list):
            raise BadRequest("`documents` must be a list")

        try:
            documents = [Document(title=str(item["title"]), text=str(item["text"])) for item in items]
        except (KeyError, TypeError) as error:
            raise BadRequest("each document needs `title` and `text`") from error

        return embedder.embed_documents(documents).tolist()

    raise BadRequest("`kind` must be `query` or `document`")


def make_handler(embedder: CachedEmbedder):
    # The model is not safe to call from several threads at once.
    model_lock = threading.Lock()

    class Handler(BaseHTTPRequestHandler):

        def do_GET(self):
            if self.path != "/health":
                self._send(404, {"error": "not found"})

                return

            self._send(
                200,
                {
                    "model": MODEL_NAME,
                    "dimension": embedder.dimension,
                    "device": embedder.embedder.device,
                    "cache_hits": embedder.hits,
                    "cache_misses": embedder.misses,
                },
            )

        def do_POST(self):
            if self.path != "/embed":
                self._send(404, {"error": "not found"})

                return

            try:
                request = self._read_json()

                with model_lock:
                    vectors = embed(embedder, request)

            except BadRequest as error:
                self._send(400, {"error": str(error)})

                return

            self._send(200, {"vectors": vectors})

        def _read_json(self) -> dict:
            length = int(self.headers.get("content-length", "0"))

            if length <= 0 or length > MAX_BODY_BYTES:
                raise BadRequest("missing or oversized request body")

            try:
                body = json.loads(self.rfile.read(length))
            except json.JSONDecodeError as error:
                raise BadRequest("the body is not valid JSON") from error

            if not isinstance(body, dict):
                raise BadRequest("the body must be a JSON object")

            return body

        def _send(self, status: int, payload: dict) -> None:
            data = json.dumps(payload).encode("utf-8")

            self.send_response(status)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def log_message(self, *args) -> None:
            # Keep the terminal quiet; the agent logs its own tool calls.
            pass

    return Handler


def main() -> None:
    parser = argparse.ArgumentParser(description="Serve EmbeddingGemma 2 over local HTTP.")

    parser.add_argument("--port", type=int, default=8765)
    parser.add_argument("--device", default=None, help="mps, cuda, or cpu (default: best available)")

    args = parser.parse_args()

    print(f"loading {MODEL_NAME} ...", flush=True)

    embedder = CachedEmbedder(Embedder(device=args.device))

    server = ThreadingHTTPServer(("127.0.0.1", args.port), make_handler(embedder))

    print(
        f"ready on http://127.0.0.1:{args.port} "
        f"({embedder.dimension} dimensions, {embedder.embedder.device})",
        flush=True,
    )

    server.serve_forever()


if __name__ == "__main__":
    main()
