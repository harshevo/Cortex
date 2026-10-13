"""Bounded host work and Apple accelerator selection."""

import os
import platform
from pathlib import Path

import numpy as np
import torch


def configure_threads() -> None:
    threads = max(1, min(4, int(os.environ.get("CORTEX_CPU_THREADS", "2"))))
    torch.set_num_threads(threads)
    try:
        torch.set_num_interop_threads(threads)
    except RuntimeError:
        pass


# Share of the machine's memory one process may take on the Apple GPU,
# which shares it with everything else that is running. Past this an
# allocation fails with an error instead of pushing the system into swap.
GPU_MEMORY_SHARE = float(os.environ.get("CORTEX_GPU_MEMORY_SHARE", "0.25"))


def gpu_device() -> str:
    configure_threads()
    if torch.backends.mps.is_available():
        torch.mps.set_per_process_memory_fraction(GPU_MEMORY_SHARE)
        return "mps"
    if torch.cuda.is_available():
        return "cuda"
    return "cpu"


def release_gpu_memory() -> None:
    """Hands back what the Apple GPU allocator keeps for reuse. It keeps a
    block for every size of batch it has seen, which adds up over a long
    run of texts of different lengths."""
    if torch.backends.mps.is_available():
        torch.mps.empty_cache()


class CoreMLContinuum:
    """A frozen HOPE continuum eligible for Apple's Neural Engine.

    Online fast-memory writes remain in PyTorch. Core ML chooses which
    supported operations actually execute on the Neural Engine.
    """

    def __init__(self, continuum, dimension: int, cache: Path | None = None):
        if platform.system() != "Darwin":
            raise RuntimeError("Core ML requires macOS")
        import hashlib
        import coremltools as ct

        configure_threads()
        self.policy = "coreml_cpu_and_neural_engine"
        model = continuum.cpu().eval()
        digest = hashlib.sha256(f"hope-coreml-v1-{dimension}".encode())
        for key, value in model.state_dict().items():
            digest.update(key.encode())
            digest.update(value.detach().cpu().numpy().tobytes())
        folder = cache or Path.home() / ".cortex" / "coreml"
        folder.mkdir(parents=True, exist_ok=True)
        path = folder / f"hope-{digest.hexdigest()[:24]}.mlpackage"
        units = ct.ComputeUnit.CPU_AND_NE
        if not path.exists():
            traced = torch.jit.trace(model, torch.zeros(1, dimension))
            converted = ct.convert(
                traced,
                inputs=[ct.TensorType(name="queries", shape=(1, dimension), dtype=np.float32)],
                outputs=[ct.TensorType(name="slow")],
                convert_to="mlprogram",
                minimum_deployment_target=ct.target.macOS13,
                compute_precision=ct.precision.FLOAT16,
                compute_units=units,
            )
            converted.save(str(path))
        self.model = ct.models.MLModel(str(path), compute_units=units)
        self.dimension = dimension
        self.devices = [type(device).__name__ for device in ct.models.MLModel.get_available_compute_devices()]

    def __call__(self, queries: torch.Tensor) -> torch.Tensor:
        rows = queries.detach().cpu().numpy().reshape(-1, self.dimension).astype(np.float32)
        predictions = [self.model.predict({"queries": row[None]})["slow"] for row in rows]
        return torch.from_numpy(np.concatenate(predictions).astype(np.float32)).to(queries.device)


class HopeRuntime:
    def __init__(self, model, accelerator: str = "auto", trained: bool = False, cache: Path | None = None):
        self.model = model
        self.device = "cpu" if accelerator == "cpu" else gpu_device()
        self.continuum = None
        self.coreml_error = None
        self.backend = f"torch_{self.device}"
        self.compute_devices = []
        if accelerator == "ane" or (accelerator == "auto" and trained and platform.system() == "Darwin"):
            try:
                self.continuum = CoreMLContinuum(model.continuum, model.config.dimension, cache)
                self.backend = f"{self.continuum.policy}+torch_{self.device}_fast_memory"
                self.compute_devices = self.continuum.devices
            except (ImportError, RuntimeError, ValueError, TypeError) as error:
                if accelerator == "ane":
                    raise RuntimeError(f"Neural Engine routing requested but unavailable: {error}") from error
                self.coreml_error = str(error)
        model.to(self.device).eval()
        model.fast.matrix = model.fast.matrix.to(self.device)
        model.fast.surprise = model.fast.surprise.to(self.device)

    def score(self, query: torch.Tensor, candidates: torch.Tensor) -> torch.Tensor:
        query, candidates = query.to(self.device), candidates.to(self.device)
        if self.continuum is None:
            return self.model.score(query, candidates).cpu()
        normalized = torch.nn.functional.normalize
        slow = self.continuum(query.unsqueeze(0))
        recalled = self.model.fast.read(normalized(query.unsqueeze(0), dim=-1))
        transformed = normalized(slow + recalled, dim=-1).squeeze(0)
        return (normalized(candidates, dim=-1) @ transformed).cpu()

    def observe(self, query: torch.Tensor, useful: torch.Tensor) -> None:
        self.model.observe(query.to(self.device), useful.to(self.device))
