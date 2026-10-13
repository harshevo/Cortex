"""Training examples for the HOPE-style memory model from public issue data.

    uv run python -m cortex_ml.hope.public --out data/hope-examples.npz \
        data/SWE-bench/data/train-00000-of-00001.parquet \
        data/SWE-rebench/data/filtered-00000-of-00001.parquet

The agent's own memory holds too few tasks to train on. Collections of
solved GitHub issues have the same shape as that memory: a task in words,
the repository it belongs to, when it was raised, and the files its fix
changed. So the same signal can be read from them: an earlier issue counts
as useful to a later one in the same repository when their fixes changed a
file in common.

Each issue is embedded twice, as the agent would: once as a new task (the
query) and once as a remembered one (its wording and the files it changed).
The vectors are saved, with each issue's repository, time and files, so
that training does not have to embed anything again.
"""

import argparse
import re
import time
from pathlib import Path

import numpy as np
import pandas as pd
import torch

from cortex_ml.hope.train import Example

# Longer issues are cut here. The instructions the agent is given are
# short; the end of a long issue is mostly logs.
MAX_CHARACTERS = 1500
MAX_TOKENS = 256

# Earlier tasks offered for each new one: the nearest by plain similarity,
# as the agent's memory search would find them.
CANDIDATES = 32

CHANGED_FILE = re.compile(r"^diff --git a/(\S+)", flags=re.MULTILINE)


def read_issues(paths: list[str]) -> pd.DataFrame:
    """Every issue that names what to do and changed at least one file."""

    frames = []

    for path in paths:
        frame = pd.read_parquet(path, columns=["repo", "created_at", "problem_statement", "patch"])

        frame["created_at"] = pd.to_datetime(frame["created_at"], utc=True)

        frames.append(frame)

    issues = pd.concat(frames, ignore_index=True)

    issues["files"] = issues["patch"].map(lambda patch: sorted(set(CHANGED_FILE.findall(patch))))
    issues["text"] = issues["problem_statement"].str.strip().str.slice(0, MAX_CHARACTERS)

    issues = issues[(issues["text"].str.len() > 0) & (issues["files"].map(len) > 0)]
    issues = issues.drop_duplicates(subset=["repo", "text"])

    return issues.sort_values(["repo", "created_at"]).reset_index(drop=True)


def embed(issues: pd.DataFrame, batch_size: int = 32) -> tuple[np.ndarray, np.ndarray]:
    """The query vector and the remembered-task vector of every issue."""

    from cortex_ml.embedding.model import Document, Embedder

    embedder = Embedder(batch_size=batch_size)

    embedder.model.max_seq_length = MAX_TOKENS

    # What the agent stores for a finished task: its instruction, the
    # files it changed and its report. There is no report here.
    remembered = [
        Document(title="earlier task", text=f"{text}\n{' '.join(files)}\n")
        for text, files in zip(issues["text"], issues["files"])
    ]

    queries = in_batches(lambda rows: embedder.embed_queries(rows), list(issues["text"]), "queries")
    documents = in_batches(lambda rows: embedder.embed_documents(rows), remembered, "remembered tasks")

    return queries, documents


def in_batches(embed_rows, rows: list, name: str, size: int = 512) -> np.ndarray:
    started = time.time()

    parts = []

    for start in range(0, len(rows), size):
        parts.append(embed_rows(rows[start : start + size]))

        done = min(start + size, len(rows))

        print(f"{name}: {done}/{len(rows)}, {done / (time.time() - started):.0f} a second", flush=True)

    return np.concatenate(parts)


def save(path: str, issues: pd.DataFrame, queries: np.ndarray, documents: np.ndarray) -> None:
    np.savez_compressed(
        path,
        queries=queries,
        documents=documents,
        repo=issues["repo"].to_numpy(dtype=str),
        files=np.array(["\n".join(files) for files in issues["files"]], dtype=str),
    )


def load_examples(path: str, candidates: int = CANDIDATES) -> dict[str, list[Example]]:
    """Ranking examples by repository, from a file written by `save`.

    Rows are in time order within a repository. A task is kept when the
    earlier tasks offered for it hold both a useful one and one that is
    not; the others have nothing to teach about ranking.
    """

    saved = np.load(path)

    queries, documents = saved["queries"], saved["documents"]
    repos = saved["repo"]
    files = [set(names.split("\n")) for names in saved["files"]]

    by_repo: dict[str, list[Example]] = {}

    boundaries = np.flatnonzero(np.r_[True, repos[1:] != repos[:-1], True])

    for first, end in zip(boundaries[:-1], boundaries[1:]):
        examples = []

        for position in range(first + 1, end):
            earlier = np.arange(first, position)

            if len(earlier) > candidates:
                nearest = np.argsort(documents[earlier] @ queries[position])[-candidates:]

                earlier = earlier[np.sort(nearest)]

            useful = [bool(files[position] & files[other]) for other in earlier]

            if not any(useful) or all(useful):
                continue

            examples.append(
                Example(
                    query=torch.from_numpy(queries[position]),
                    candidates=torch.from_numpy(documents[earlier]),
                    useful=torch.tensor(useful),
                )
            )

        if examples:
            by_repo[str(repos[first])] = examples

    return by_repo


def main() -> None:
    parser = argparse.ArgumentParser(description="Embed public issue data for training the memory model.")

    parser.add_argument("parquet", nargs="+", help="issue collections with repo, created_at, problem_statement, patch")
    parser.add_argument("--out", default="data/hope-examples.npz")

    args = parser.parse_args()

    issues = read_issues(args.parquet)

    print(f"{len(issues)} issues in {issues['repo'].nunique()} repositories", flush=True)

    queries, documents = embed(issues)

    Path(args.out).parent.mkdir(parents=True, exist_ok=True)

    save(args.out, issues, queries, documents)

    print(f"saved {args.out}")


if __name__ == "__main__":
    main()
