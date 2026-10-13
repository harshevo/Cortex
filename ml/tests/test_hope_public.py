import numpy as np
import pandas as pd

from cortex_ml.hope.pretrain import quality, split
from cortex_ml.hope.model import HopeConfig, HopeMemory
from cortex_ml.hope.public import load_examples, read_issues, save


def patch(*files: str) -> str:
    return "".join(f"diff --git a/{name} b/{name}\n--- a/{name}\n+++ b/{name}\n" for name in files)


def test_issues_become_ranking_examples_in_time_order(tmp_path):
    rows = [
        ("a/one", "2024-01-03", "third: fix the parser again", patch("parser.py")),
        ("a/one", "2024-01-01", "first: fix the parser", patch("parser.py", "util.py")),
        ("a/one", "2024-01-02", "second: document the cli", patch("README.md")),
        ("b/two", "2024-01-01", "alone in its repository", patch("x.py")),
        ("a/one", "2024-01-04", "", patch("y.py")),
    ]

    frame = pd.DataFrame(rows, columns=["repo", "created_at", "problem_statement", "patch"])

    frame.to_parquet(tmp_path / "issues.parquet")

    issues = read_issues([str(tmp_path / "issues.parquet")])

    assert list(issues["text"].str.split(":").str[0]) == ["first", "second", "third", "alone in its repository"]
    assert issues["files"][0] == ["parser.py", "util.py"]

    vectors = np.eye(4, dtype=np.float32)

    save(str(tmp_path / "examples.npz"), issues, vectors, vectors)

    by_repo = load_examples(str(tmp_path / "examples.npz"))

    # Only the third issue has both a useful and a not-useful earlier one.
    assert list(by_repo) == ["a/one"]

    (example,) = by_repo["a/one"]

    assert example.useful.tolist() == [True, False]
    assert example.candidates.shape == (2, 4)

    training, validation, test = split(by_repo)

    assert len(training) + len(validation) + len(test) == 1

    first, rank = quality(HopeMemory(HopeConfig(dimension=4, hidden=8)), [example])

    assert first in (0.0, 1.0) and 0.5 <= rank <= 1.0
