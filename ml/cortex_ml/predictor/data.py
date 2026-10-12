"""Where the predictor's training examples come from.

An example is a file before a change, a description of the change, and the
file after it. Two sources supply them:

- A repository's git history: every commit that modified a file. This is
  available before the agent has run at all.
- The agent's own trajectories: every successful `edit_file` call, with
  the task as the description. This grows as the agent is used.
"""

import json
import subprocess
from dataclasses import dataclass
from pathlib import Path

# Embedding models read a limited window; longer text is cut.
MAX_CHARS = 6_000

CODE_SUFFIXES = (".py", ".ts", ".tsx", ".js", ".jsx", ".rs")


@dataclass(frozen=True)
class Edit:
    path: str
    before: str
    action: str
    after: str


def _git(repo: str, *args: str) -> str:
    result = subprocess.run(
        ["git", "-C", repo, *args],
        capture_output=True,
        text=True,
        errors="replace",
        check=False,
    )

    return result.stdout if result.returncode == 0 else ""


def edits_from_git(repo: str, limit: int = 2_000) -> list[Edit]:
    """One example per modified source file per commit, oldest first."""

    edits = []

    commits = _git(repo, "log", "--reverse", "--format=%H").split()

    for commit in commits:
        subject = _git(repo, "log", "-1", "--format=%s", commit).strip()

        # `M` keeps files that existed before and after the commit.
        changed = _git(repo, "diff", "--name-only", "--diff-filter=M", f"{commit}~1", commit)

        for path in changed.splitlines():
            if not path.endswith(CODE_SUFFIXES):
                continue

            before = _git(repo, "show", f"{commit}~1:{path}")
            after = _git(repo, "show", f"{commit}:{path}")
            diff = _git(repo, "diff", "--unified=0", f"{commit}~1", commit, "--", path)

            if not before or not after:
                continue

            edits.append(
                Edit(
                    path=path,
                    before=before[:MAX_CHARS],
                    action=f"{subject}\n{diff}"[:MAX_CHARS],
                    after=after[:MAX_CHARS],
                )
            )

            if len(edits) >= limit:
                return edits

    return edits


def edits_from_trajectories(directory: str) -> list[Edit]:
    """One example per successful `edit_file` call in the run logs."""

    edits = []

    for file in sorted(Path(directory).rglob("*.jsonl")):
        instruction = ""
        pending: dict[str, dict] = {}

        for line in file.read_text(errors="replace").splitlines():
            try:
                event = json.loads(line)
            except json.JSONDecodeError:
                continue

            kind = event.get("type")

            if kind == "run_started":
                instruction = event.get("instruction", "")

            elif kind == "tool_started" and event["call"]["name"] == "edit_file":
                pending[event["call"]["id"]] = event["call"]["input"]

            elif kind == "tool_finished" and not event["result"]["is_error"]:
                call = pending.pop(event["result"]["call_id"], None)

                if call is None or "old_text" not in call or "new_text" not in call:
                    continue

                edits.append(
                    Edit(
                        path=str(call.get("path", "")),
                        before=str(call["old_text"])[:MAX_CHARS],
                        action=instruction[:MAX_CHARS],
                        after=str(call["new_text"])[:MAX_CHARS],
                    )
                )

    return edits
