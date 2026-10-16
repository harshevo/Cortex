"""Bounded host work and Apple accelerator selection."""

import os

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


