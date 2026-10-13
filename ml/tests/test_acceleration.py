import pytest
import torch

from cortex_ml.acceleration import HopeRuntime, gpu_device
from cortex_ml.hope.model import HopeMemory


def vectors(count: int, dimension: int) -> torch.Tensor:
    torch.manual_seed(7)

    return torch.randn(count, dimension)


def test_the_accelerated_model_scores_like_the_plain_one():
    reference = HopeMemory().eval()
    dimension = reference.config.dimension

    query, candidates = vectors(1, dimension)[0], vectors(6, dimension)

    with torch.no_grad():
        expected = reference.score(query, candidates)

        for accelerator in ["cpu", "gpu"]:
            runtime = HopeRuntime(HopeMemory().eval(), accelerator=accelerator)

            assert runtime.backend == ("torch_cpu" if accelerator == "cpu" else f"torch_{gpu_device()}")
            assert torch.allclose(runtime.score(query, candidates), expected, atol=1e-4)


def test_learning_in_a_task_still_works_on_the_accelerator():
    runtime = HopeRuntime(HopeMemory().eval(), accelerator="gpu")
    dimension = runtime.model.config.dimension

    query, candidates = vectors(1, dimension)[0], vectors(6, dimension)

    with torch.no_grad():
        before = runtime.score(query, candidates)
        runtime.observe(query, candidates[3])
        after = runtime.score(query, candidates)

    assert after[3] > before[3]


def test_the_neural_engine_route_agrees_with_torch(tmp_path):
    pytest.importorskip("coremltools")

    model = HopeMemory().eval()
    dimension = model.config.dimension

    query, candidates = vectors(1, dimension)[0], vectors(6, dimension)

    with torch.no_grad():
        expected = HopeMemory().eval().score(query, candidates)

        runtime = HopeRuntime(model, accelerator="ane", cache=tmp_path)

        assert runtime.backend.startswith("coreml_cpu_and_neural_engine")
        # Core ML computes in half precision.
        assert torch.allclose(runtime.score(query, candidates), expected, atol=2e-2)
