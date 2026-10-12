"""Measure semantic search on its own, without a language model.

For every task in a suite, the task's instruction is used as the search
query against the project exactly as the agent would see it. A task counts
as found at rank N when the Nth result is in a file the reference solution
changes.

The embedding service must be running (see `ml/`). Run from the cortex
directory:

    python3 eval/tools/retrieval_accuracy.py eval/suites/tradelyzer
"""

import argparse
import re
import shutil
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path

CORTEX_DIR = Path(__file__).resolve().parents[2]
CORTEX_BINARY = CORTEX_DIR / "target" / "debug" / "cortex"

MAX_RESULTS = 10

# A result line looks like: `0.812  src/lib/money.ts:4-15  src/lib/money.ts > toCents`
RESULT_LINE = re.compile(r"^\s*([\d.-]+)\s+(\S+?):(\d+)-(\d+)\s+(.*)$")


def changed_files(task_dir: Path) -> set[str]:
    """Return the paths the reference solution touches."""

    solution = task_dir / "solution"

    return {str(path.relative_to(solution)) for path in solution.rglob("*") if path.is_file()}


def prepare_working_copy(task_dir: Path, task: dict, destination: Path) -> None:
    """Build the project the way the eval harness does for the agent."""

    source = task["source"]

    repo = (task_dir / source["repo"]).resolve()

    archive = subprocess.run(
        ["git", "-C", str(repo), "archive", "--format=tar", source["commit"]],
        capture_output=True,
        check=True,
    )

    subprocess.run(["tar", "-x", "-C", str(destination)], input=archive.stdout, check=True)

    for removed in source.get("remove", []):
        path = destination / removed

        if path.is_dir():
            shutil.rmtree(path)
        elif path.exists():
            path.unlink()

    overlay = task_dir / "repo"

    if overlay.is_dir():
        shutil.copytree(overlay, destination, dirs_exist_ok=True)


def search(query: str, directory: Path) -> list[tuple[float, str, str]]:
    """Return (score, path, title) for each result, best first."""

    completed = subprocess.run(
        [str(CORTEX_BINARY), "search", query, "--dir", str(directory), "--max-results", str(MAX_RESULTS)],
        capture_output=True,
        text=True,
    )

    if completed.returncode != 0:
        sys.exit(f"cortex search failed: {completed.stderr.strip()}")

    results = []

    for line in completed.stdout.splitlines():
        match = RESULT_LINE.match(line)

        if match:
            results.append((float(match.group(1)), match.group(2), match.group(5)))

    return results


def rank_of_target(results: list[tuple[float, str, str]], targets: set[str]) -> int | None:
    for position, (_, path, _) in enumerate(results, start=1):
        if path in targets:
            return position

    return None


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])

    parser.add_argument("suite", type=Path)

    args = parser.parse_args()

    if not CORTEX_BINARY.exists():
        sys.exit("build cortex first: cargo build")

    ranks: list[int | None] = []

    for task_dir in sorted(path for path in args.suite.iterdir() if (path / "task.toml").is_file()):
        task = tomllib.loads((task_dir / "task.toml").read_text(encoding="utf-8"))

        targets = changed_files(task_dir)

        if "source" not in task or not targets:
            continue

        with tempfile.TemporaryDirectory(prefix="cortex-retrieval-") as temporary:
            working_copy = Path(temporary)

            prepare_working_copy(task_dir, task, working_copy)

            results = search(task["instruction"], working_copy)

        rank = rank_of_target(results, targets)

        ranks.append(rank)

        top = results[0][2] if results else "(no results)"

        shown = str(rank) if rank else f">{MAX_RESULTS}"

        print(f"  {task_dir.name:<28} rank {shown:<4} top result: {top}")

    if not ranks:
        sys.exit("no tasks with a source repository and a solution were found")

    total = len(ranks)

    def found_within(limit: int) -> int:
        return sum(1 for rank in ranks if rank is not None and rank <= limit)

    reciprocal = sum(1 / rank for rank in ranks if rank is not None) / total

    print()
    print(f"  tasks: {total}")
    print(f"  right file is the first result:   {found_within(1)} of {total}")
    print(f"  right file in the first 3:        {found_within(3)} of {total}")
    print(f"  right file in the first 5:        {found_within(5)} of {total}")
    print(f"  mean reciprocal rank:             {reciprocal:.2f}")


if __name__ == "__main__":
    main()
