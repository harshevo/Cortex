"""Generate the `tradelyzer-long` evaluation suite.

The `tradelyzer` suite plants one defect per task, and the plain agent solves
every one of them, so it cannot show whether a change makes the agent better.
This suite plants several of the same defects at once and reports them
together, the way a backlog arrives. A task is then long: five or ten separate
problems in up to six files, to be found, fixed and kept track of in one run.

Each defect is one part of its task, with its own check: the project tests
that fail because of that defect and no other. An attempt that fixes three of
five scores 60%, so two configurations can be told apart even when neither
finishes the whole task.

Run from the cortex directory:

    python3 eval/tools/make_tradelyzer_long_suite.py

The script rewrites `eval/suites/tradelyzer-long/` from scratch. It runs the
project's tests many times to work out which tests belong to which defect and
to confirm that fixing one defect passes that defect's part and no other, so
it takes a few minutes. Afterwards, check the suite with
`cortex eval validate --suite eval/suites/tradelyzer-long`.
"""

import json
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

import make_tradelyzer_suite as single

SUITE_DIR = single.CORTEX_DIR / "eval" / "suites" / "tradelyzer-long"
DEPENDENCIES = single.CORTEX_DIR / "eval" / "sources" / "tradelyzer-deps"

# Which defects are planted together. A defect is left out of a task, with a
# message, when its tests cannot be told apart from another's in that task.
LONG_TASKS = {
    "long-imports": [
        "partial-exit-fees",
        "same-millisecond-order",
        "reversal-opening-lost",
        "cent-rounding-drift",
        "bybit-repeating-cursor",
    ],
    "long-journal": [
        "calendar-evening-trades",
        "calendar-missing-week",
        "negative-amounts",
        "cross-day-screenshot",
        "legacy-tag-overflow",
    ],
    "long-mixed-three": [
        "partial-exit-fees",
        "calendar-missing-week",
        "negative-amounts",
    ],
    "long-mixed-four": [
        "bybit-repeating-cursor",
        "cross-day-screenshot",
        "calendar-evening-trades",
        "cent-rounding-drift",
    ],
    "long-everything": [
        "partial-exit-fees",
        "same-millisecond-order",
        "reversal-opening-lost",
        "cent-rounding-drift",
        "bybit-repeating-cursor",
        "calendar-evening-trades",
        "calendar-missing-week",
        "negative-amounts",
        "cross-day-screenshot",
        "legacy-tag-overflow",
    ],
}

# Model calls allowed: a base, plus what one defect took the plain agent.
STEPS_BASE = 20
STEPS_PER_DEFECT = 12

INTRODUCTION = (
    "Several problems have been reported in this project. They are separate; fix all of "
    "them. Keep track of which you have dealt with, and check each fix before moving on."
)

DEFECTS = {task.name: task for task in single.TASKS}

# A test, as (file, full name).
Test = tuple[str, str]


def plant(names: list[str]) -> dict[str, str]:
    """Return the files changed by planting every defect in `names`."""

    changed: dict[str, str] = {}

    for name in names:
        for edit in DEFECTS[name].edits:
            text = changed.get(edit.path) or single.read_source(edit.path)

            if text.count(edit.old) != 1:
                sys.exit(f"{name}: its edit to {edit.path} no longer fits beside the others")

            changed[edit.path] = text.replace(edit.old, edit.new)

    return changed


def failing_tests(planted: dict[str, str]) -> set[Test]:
    """Run the project's tests with `planted` in place; return those that fail."""

    with tempfile.TemporaryDirectory(prefix="cortex-long-") as directory:
        root = Path(directory)

        archive = subprocess.Popen(
            ["git", "-C", str(single.SOURCE_REPO), "archive", "--format=tar", single.COMMIT],
            stdout=subprocess.PIPE,
        )

        subprocess.run(["tar", "-x", "-C", str(root)], stdin=archive.stdout, check=True)

        archive.wait()

        (root / "node_modules").symlink_to(DEPENDENCIES / "node_modules")

        for path, text in planted.items():
            single.write_file(root / path, text)

        report = root / "report.json"

        subprocess.run(
            ["npx", "vitest", "run", "--reporter=json", f"--outputFile={report}"],
            cwd=root,
            capture_output=True,
            check=False,
        )

        results = json.loads(report.read_text(encoding="utf-8"))

        failed: set[Test] = set()

        for file in results["testResults"]:
            # Reported as an absolute path inside the temporary directory.
            relative = str(Path(file["name"]).resolve().relative_to(root.resolve()))

            for test in file["assertionResults"]:
                if test["status"] != "passed":
                    failed.add((relative, test["fullName"]))

            # A file that could not be loaded has no tests listed.
            if file["status"] != "passed" and not file["assertionResults"]:
                sys.exit(f"{relative} could not be run: {file.get('message', '')}")

        return failed


def milestone_command(tests: set[Test]) -> str:
    """The command that passes when every test in `tests` passes."""

    files = sorted({file for file, _ in tests})

    names = "|".join(sorted(f"^{re.escape(name)}$" for _, name in tests))

    return shlex.join(["npx", "vitest", "run", *files, "-t", names])


def parts_of(task_name: str, names: list[str], alone: dict[str, set[Test]]) -> dict[str, set[Test]]:
    """The tests that tell each defect of a task apart from the others."""

    parts: dict[str, set[Test]] = {}

    for name in names:
        others = set().union(*(alone[other] for other in names if other != name))

        own = alone[name] - others

        if own:
            parts[name] = own
        else:
            print(f"{task_name}: leaving out {name}, no test fails for it alone")

    return parts


def confirm_independent(task_name: str, parts: dict[str, set[Test]]) -> None:
    """Fix each defect in turn and check that only its own part passes."""

    names = list(parts)

    for fixed in names:
        failed = failing_tests(plant([name for name in names if name != fixed]))

        if parts[fixed] & failed:
            sys.exit(f"{task_name}: the part for {fixed} still fails once it is fixed")

        for other in names:
            if other != fixed and not parts[other] <= failed:
                sys.exit(f"{task_name}: fixing {fixed} also passes part of {other}")


def write_task(task_name: str, parts: dict[str, set[Test]], tests: list[str]) -> None:
    task_dir = SUITE_DIR / task_name

    names = list(parts)

    for path, text in plant(names).items():
        single.write_file(task_dir / "repo" / path, text)
        single.write_file(task_dir / "solution" / path, single.read_source(path))

    for path in tests:
        single.write_file(task_dir / "hidden" / path, single.read_source(path))

    reports = [f"{position}. {DEFECTS[name].instruction}" for position, name in enumerate(names, 1)]

    instruction = "\n\n".join([INTRODUCTION, *reports])

    hidden: list[str] = []

    for name in names:
        for path in DEFECTS[name].hidden_tests:
            if path not in hidden:
                hidden.append(path)

    lines = [
        f"instruction = {single.toml_string(instruction)}",
        f"max_steps = {STEPS_BASE + STEPS_PER_DEFECT * len(names)}",
        "",
        "[source]",
        f"repo = {single.toml_string(single.SOURCE_FROM_TASK)}",
        f"commit = {single.toml_string(single.COMMIT)}",
        f"remove = {single.toml_list(hidden)}",
        'link = ["node_modules"]',
        f"link_from = {single.toml_string(single.DEPENDENCIES_FROM_TASK)}",
        "",
        "[check]",
        f"command = {single.toml_string(single.CHECK_COMMAND)}",
    ]

    for name in names:
        lines += [
            "",
            "[[milestone]]",
            f"name = {single.toml_string(name)}",
            f"command = {single.toml_string(milestone_command(parts[name]))}",
        ]

    single.write_file(task_dir / "task.toml", "\n".join(lines) + "\n")


def main() -> None:
    if not single.SOURCE_REPO.is_dir():
        sys.exit(f"clone tradelyzer into {single.SOURCE_REPO} first")

    if failing_tests({}):
        sys.exit("the project's tests fail before any defect is planted")

    used = sorted({name for names in LONG_TASKS.values() for name in names})

    print(f"finding the tests that fail for each of {len(used)} defects")

    alone = {name: failing_tests(plant([name])) for name in used}

    for name, failed in alone.items():
        if not failed:
            sys.exit(f"{name}: no test fails with this defect planted")

    if SUITE_DIR.exists():
        shutil.rmtree(SUITE_DIR)

    tests = single.list_tests()

    for task_name, names in LONG_TASKS.items():
        parts = parts_of(task_name, names, alone)

        print(f"{task_name}: checking that its {len(parts)} parts are independent")

        confirm_independent(task_name, parts)

        write_task(task_name, parts, tests)

    print(f"wrote {len(LONG_TASKS)} tasks to {SUITE_DIR}")


if __name__ == "__main__":
    main()
