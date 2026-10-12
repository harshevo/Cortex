"""Serves the HOPE-style memory model to the Rust agent.

    uv run python -m cortex_ml.hope.server --port 8766 --weights hope.pt

Endpoints:

    GET  /health    -> {"model": "hope", "trained": true, "dimension": ...}
    POST /rerank    <- {"query": [...], "candidates": [[...], ...]}
                    -> {"scores": [...]}
    POST /observe   <- {"query": [...], "useful": [...]}
                    -> {"ok": true}

`/observe` is the in-task learning step: it writes one association into the
fast memory. It listens on 127.0.0.1 only.
"""

import argparse
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import torch

from cortex_ml.hope.model import HopeConfig, HopeMemory


def load(weights: str | None) -> tuple[HopeMemory, bool]:
    """The trained model, or an untrained one that ranks like cosine."""

    if weights is None:
        return HopeMemory(), False

    saved = torch.load(weights, map_location="cpu")

    config = dict(saved["config"])
    config["periods"] = tuple(config["periods"])

    model = HopeMemory(HopeConfig(**config))

    model.load_state_dict(saved["state"])

    return model, True


def make_handler(model: HopeMemory, trained: bool):
    lock = threading.Lock()

    class Handler(BaseHTTPRequestHandler):

        def do_GET(self):
            if self.path != "/health":
                return self._send(404, {"error": "not found"})

            self._send(200, {"model": "hope", "trained": trained, "dimension": model.config.dimension})

        def do_POST(self):
            try:
                length = int(self.headers.get("content-length", "0"))
                body = json.loads(self.rfile.read(length))

                query = torch.tensor(body["query"], dtype=torch.float32)

                with lock, torch.no_grad():
                    if self.path == "/rerank":
                        candidates = torch.tensor(body["candidates"], dtype=torch.float32)

                        return self._send(200, {"scores": model.score(query, candidates).tolist()})

                    if self.path == "/observe":
                        model.observe(query, torch.tensor(body["useful"], dtype=torch.float32))

                        return self._send(200, {"ok": True})

            except (KeyError, ValueError, RuntimeError, json.JSONDecodeError) as error:
                return self._send(400, {"error": str(error)})

            self._send(404, {"error": "not found"})

        def _send(self, status: int, payload: dict) -> None:
            data = json.dumps(payload).encode("utf-8")

            self.send_response(status)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def log_message(self, *args) -> None:
            pass

    return Handler


def main() -> None:
    parser = argparse.ArgumentParser(description="Serve the HOPE-style memory model.")

    parser.add_argument("--port", type=int, default=8766)
    parser.add_argument("--weights", default=None, help="file written by cortex_ml.hope.train")

    args = parser.parse_args()

    model, trained = load(args.weights)

    model.eval()

    server = ThreadingHTTPServer(("127.0.0.1", args.port), make_handler(model, trained))

    state = "trained" if trained else "untrained (ranks like plain cosine)"

    print(f"ready on http://127.0.0.1:{args.port}, {state}", flush=True)

    server.serve_forever()


if __name__ == "__main__":
    main()
