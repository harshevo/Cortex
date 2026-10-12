"""Generate the `tradelyzer` evaluation suite.

Each task plants one realistic defect in the real tradelyzer code base and
describes it the way a user would report it. The project's own tests score
the result; the test file that covers the defect is hidden from the agent.

Run from the cortex directory:

    python3 eval/tools/make_tradelyzer_suite.py

The script rewrites `eval/suites/tradelyzer/` from scratch. After running it,
check the suite with `cortex eval validate --suite eval/suites/tradelyzer`.
"""

import shutil
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path

CORTEX_DIR = Path(__file__).resolve().parents[2]
SOURCE_REPO = CORTEX_DIR / "eval" / "sources" / "tradelyzer"
SUITE_DIR = CORTEX_DIR / "eval" / "suites" / "tradelyzer"

# The tasks are written against this exact commit.
COMMIT = "1e606dc257a4172721171baa75c22c376dd09dfe"

# Paths from a task directory back to the source repository and to the
# directory holding its installed dependencies. The dependencies live outside
# the repository so the link in a working copy does not lead back to the
# original code and tests.
SOURCE_FROM_TASK = "../../../sources/tradelyzer"
DEPENDENCIES_FROM_TASK = "../../../sources/tradelyzer-deps"

CHECK_COMMAND = "npx vitest run"

FILL_MATCHING = "src/features/imports/server/fill-matching.ts"
EXCHANGE_SERVICE = "src/features/imports/server/exchange-service.ts"
CALENDAR = "src/features/calendar/utils/calendar.ts"
MONEY = "src/lib/money.ts"
JOURNAL_SCHEMA = "src/features/journal/schemas/journal.schema.ts"
JOURNAL_DATABASE = "src/lib/db/journal-database.ts"
TRADE_FILE = "src/features/trades/schemas/trade-file.schema.ts"

EXCHANGE_TESTS = "tests/exchanges.test.ts"
CALENDAR_TESTS = "tests/calendar.test.ts"
DATABASE_TESTS = "tests/journal-database.test.ts"
JOURNAL_TESTS = "tests/journal.test.ts"
TRADE_FILE_TESTS = "tests/trade-file.test.ts"


@dataclass
class Edit:
    """Replace `old` with `new` in `path`. `old` must appear exactly once."""

    path: str
    old: str
    new: str


@dataclass
class Task:
    name: str
    instruction: str
    edits: list[Edit]

    # Test files removed from the agent's view.
    hidden_tests: list[str] = field(default_factory=list)


TASKS = [
    Task(
        name="partial-exit-fees",
        instruction=(
            "Users importing exchange history report that when a position is closed in "
            "several partial exits, the commissions on the resulting trades add up to more "
            "than the fees the exchange actually charged. Trades closed in a single exit are "
            "correct. Find the cause in the import code and fix it."
        ),
        edits=[
            Edit(
                FILL_MATCHING,
                "      lot.quantity = lot.quantity.minus(size);\n"
                "      lot.fee = lot.fee.minus(entryFee);\n",
                "      lot.quantity = lot.quantity.minus(size);\n",
            ),
        ],
        hidden_tests=[EXCHANGE_TESTS],
    ),
    Task(
        name="same-millisecond-order",
        instruction=(
            "Bybit sometimes reports an entry fill and its exit fill with the same "
            "millisecond timestamp. Bybit also supplies a sequence number that gives the true "
            "order of executions, but imports of such round trips come out wrong: the exit is "
            "processed before the entry, so the trade is reported as unmatched or with the "
            "wrong profit. Execution ids are random and say nothing about order. Fix the "
            "import so that fills with the same timestamp are processed in their true order."
        ),
        edits=[
            Edit(
                FILL_MATCHING,
                "  const leftOrder = left.sequence ?? left.id;\n"
                "  const rightOrder = right.sequence ?? right.id;\n",
                "  const leftOrder = left.id;\n"
                "  const rightOrder = right.id;\n",
            ),
        ],
        hidden_tests=[EXCHANGE_TESTS],
    ),
    Task(
        name="reversal-opening-lost",
        instruction=(
            "A futures import can start in the middle of a position: the first fill in the "
            "selected date range closes a position that was opened before the range. That "
            "close is correctly counted as unmatched. But when that same fill is a reversal, "
            "meaning the exchange says only part of it closed the old position and the rest "
            "opened a new position in the other direction, the new position is lost. Its "
            "later exit is then also reported as unmatched instead of producing a trade. Fix "
            "the import so the opening part of such a fill is kept. This applies to futures "
            "only; spot imports never open a position on a sell."
        ),
        edits=[
            Edit(
                FILL_MATCHING,
                "      const opening = quantity.minus(closing);\n"
                "\n"
                "      if (explicit && opening.gt(0) && !spot) {\n"
                "        lots.push({\n"
                "          fill,\n"
                "          quantity: opening,\n"
                "          fee: toDecimal(fill.fee).mul(opening).div(quantity),\n"
                "        });\n"
                "      }\n"
                "\n"
                "      continue;\n",
                "      continue;\n",
            ),
        ],
        hidden_tests=[EXCHANGE_TESTS],
    ),
    Task(
        name="cent-rounding-drift",
        instruction=(
            "Imported trades have their commission and net P&L rounded to cents one trade at "
            "a time, so rounding errors pile up. Example: one exit with a $0.01 fee that "
            "closes three entry lots produces three trades that each show a commission of "
            "0.00, and the cent disappears from the totals.\n\n"
            "Change `matchFills` so that rounding is carried across the whole import. Keep "
            "exact running totals of commission and of net P&L over all trades produced so "
            "far, in the order they are produced. Each trade's displayed commission is the "
            "running commission total rounded to cents (half up) minus the rounded running "
            "total as it stood before that trade; net P&L works the same way. The displayed "
            "values then always add up to the rounded exact total."
        ),
        edits=[
            Edit(
                FILL_MATCHING,
                "  let exactCommission = toDecimal(\"0\");\n"
                "  let exactNet = toDecimal(\"0\");\n"
                "  let roundedCommission = toDecimal(\"0\");\n"
                "  let roundedNet = toDecimal(\"0\");\n",
                "",
            ),
            Edit(
                FILL_MATCHING,
                "      exactCommission = exactCommission.plus(commission);\n"
                "      exactNet = exactNet.plus(gross.plus(commission));\n"
                "\n"
                "      const nextCommission = toDecimal(toMoneyString(exactCommission));\n"
                "      const nextNet = toDecimal(toMoneyString(exactNet));\n"
                "      const allocatedCommission = nextCommission.minus(roundedCommission);\n"
                "      const allocatedNet = nextNet.minus(roundedNet);\n"
                "\n"
                "      roundedCommission = nextCommission;\n"
                "      roundedNet = nextNet;\n",
                "      const allocatedCommission = commission;\n"
                "      const allocatedNet = gross.plus(commission);\n",
            ),
        ],
        hidden_tests=[EXCHANGE_TESTS],
    ),
    Task(
        name="binance-closing-quantities",
        instruction=(
            "`withBinanceClosingQuantities` in the import code is not implemented. Implement "
            "it.\n\n"
            "Background: Binance futures fills in one-way mode (lane `BOTH`) do not say how "
            "much of each fill closed an existing position. The function receives the fills "
            "and, per symbol, the signed position after the newest fill (positive is long, "
            "negative is short), and works backwards to fill in `closed`.\n\n"
            "Rules:\n"
            "- Fills are identified by symbol and id together. Duplicates count once.\n"
            "- Walk the distinct fills from newest to oldest, ordered by time and then by id "
            "compared as an integer.\n"
            "- Only fills whose lane is `BOTH` are processed; others are left untouched.\n"
            "- For each processed fill: the position before it is the position after it "
            "minus the quantity for a BUY, or plus the quantity for a SELL. A BUY closes when "
            "the position before was short; a SELL closes when it was long. The closed "
            "quantity is then the smaller of the fill quantity and the size of the position "
            "before; otherwise it is zero. The position before becomes the position after for "
            "the next (older) fill of that symbol.\n"
            "- Return the fills in their original order and count, with each processed fill "
            "replaced by a copy whose `closed` is the closed quantity as a decimal string.\n"
            "- Use the file's 40-digit decimal type for all arithmetic.\n\n"
            "Errors, checked per processed fill in this order:\n"
            "- quantity not finite or not positive: `Exchange returned an invalid fill.`\n"
            "- no finite ending position for the fill's symbol: `Binance position state is "
            "missing. Retry the import or use a file export.`\n"
            "- closed quantity is zero but the fill reports a non-zero `pnl`: `Binance "
            "position history could not be reconciled. Retry when trading is paused or use a "
            "file export.`"
        ),
        edits=[],
        hidden_tests=[EXCHANGE_TESTS],
    ),
    Task(
        name="bybit-repeating-cursor",
        instruction=(
            "Bybit's execution history is read page by page using a cursor. If Bybit returns "
            "a cursor it has already returned, the import keeps requesting pages until it "
            "runs out of its request budget and then fails with a misleading error. It should "
            "instead stop as soon as a cursor repeats, with the error `Bybit pagination did "
            "not advance. Retry with a smaller range.`"
        ),
        edits=[
            Edit(
                EXCHANGE_SERVICE,
                "  to: number,\n"
                "): Promise<void> {\n"
                "  const seenCursors = new Set<string>();\n"
                "  let cursor = \"\";\n",
                "  to: number,\n"
                "): Promise<void> {\n"
                "  let cursor = \"\";\n",
            ),
            Edit(
                EXCHANGE_SERVICE,
                "    cursor = data.result.nextPageCursor ?? \"\";\n"
                "\n"
                "    if (cursor && seenCursors.has(cursor)) {\n"
                "      throw new Error(\n"
                "        \"Bybit pagination did not advance. Retry with a smaller range.\",\n"
                "      );\n"
                "    }\n"
                "\n"
                "    seenCursors.add(cursor);\n",
                "    cursor = data.result.nextPageCursor ?? \"\";\n",
            ),
        ],
        hidden_tests=[EXCHANGE_TESTS],
    ),
    Task(
        name="calendar-evening-trades",
        instruction=(
            "The trade calendar should file each trade under the day it closed in the time "
            "zone the user has chosen. A user whose calendar is set to New York reports that "
            "trades closed in the evening appear on the following day. Fix it."
        ),
        edits=[
            Edit(
                CALENDAR,
                "DateTime.fromISO(trade.closedAt, { zone })",
                "DateTime.fromISO(trade.closedAt, { setZone: true })",
            ),
        ],
        hidden_tests=[CALENDAR_TESTS],
    ),
    Task(
        name="calendar-missing-week",
        instruction=(
            "The month view of the trade calendar shows whole weeks, Monday to Sunday, "
            "including the days of neighbouring months that fall in the first and last week. "
            "In months whose last day is not a Sunday, the final week is missing, so the last "
            "days of the month cannot be seen. Fix it."
        ),
        edits=[
            Edit(
                CALENDAR,
                "  const end = last.plus({ days: 7 - last.weekday }).startOf(\"day\");\n",
                "  const end = last.startOf(\"day\");\n",
            ),
            Edit(
                CALENDAR,
                "day <= end; day = day.plus({ weeks: 1 })",
                "day.plus({ days: 6 }) <= end; day = day.plus({ weeks: 1 })",
            ),
        ],
        hidden_tests=[CALENDAR_TESTS],
    ),
    Task(
        name="negative-amounts",
        instruction=(
            "Losing trades with cents are displayed wrongly throughout the app. A net P&L of "
            "-1.50 shows as a loss of 0.50, and -0.01 shows as a gain of one cent. Whole "
            "dollar losses and all gains are fine. Find the cause and fix it."
        ),
        edits=[
            Edit(
                MONEY,
                "  const isNegative = value.startsWith(\"-\");\n"
                "  const [whole, fraction = \"\"] = value.replace(/^-/, \"\").split(\".\");\n"
                "  const cents =\n"
                "    BigInt(whole) * CENTS_PER_UNIT + BigInt(fraction.padEnd(2, \"0\"));\n"
                "\n"
                "  if (isNegative) {\n"
                "    return -cents;\n"
                "  }\n"
                "\n"
                "  return cents;\n",
                "  const [whole, fraction = \"\"] = value.split(\".\");\n"
                "\n"
                "  return BigInt(whole) * CENTS_PER_UNIT + BigInt(fraction.padEnd(2, \"0\"));\n",
            ),
        ],
        hidden_tests=[TRADE_FILE_TESTS, JOURNAL_TESTS],
    ),
    Task(
        name="cross-day-screenshot",
        instruction=(
            "When a day's journal is saved, the caller passes the ids of screenshots to "
            "remove. Nothing checks that those screenshots belong to the day being saved, so "
            "saving one day's journal can delete another day's screenshot. Make the save "
            "refuse, with the error `Screenshot belongs to another day.`, when any screenshot "
            "to remove belongs to a different day. In that case nothing may be saved or "
            "deleted. An id that matches no stored screenshot is not an error."
        ),
        edits=[
            Edit(
                JOURNAL_DATABASE,
                "        for (const id of changes.removed) {\n"
                "          const screenshot = await this.screenshots.get(id);\n"
                "          if (screenshot && screenshot.date !== validated.date)\n"
                "            throw new Error(\"Screenshot belongs to another day.\");\n"
                "        }\n",
                "",
            ),
        ],
        hidden_tests=[DATABASE_TESTS],
    ),
    Task(
        name="legacy-tag-overflow",
        instruction=(
            "A journal entry may have at most 10 tags. Some entries saved by an older "
            "version have more, up to 30. Such an entry can no longer be edited at all, "
            "because every save fails the 10-tag rule.\n\n"
            "Change `journalSchemaFor(previous)` so that when the previously saved entry has "
            "more than 10 tags, the new version is accepted if it has at most 10 tags, or if "
            "every one of its tags was already on the saved entry (tags may be kept or "
            "removed, not added), and never more than 30. When that rule is broken the "
            "message must be `Choose up to 10 mistake tags. Existing tags can be kept or "
            "removed.` Entries with no saved version, or whose saved version has 10 tags or "
            "fewer, keep the plain 10-tag limit and its existing message."
        ),
        edits=[
            Edit(
                JOURNAL_SCHEMA,
                "  if (!previous || previous.tags.length <= MAX_JOURNAL_TAGS)\n"
                "    return journalSchema;\n"
                "  return journalSchema.extend({\n"
                "    tags: z\n"
                "      .array(journalSchema.shape.tags.element)\n"
                "      .max(30)\n"
                "      .refine(\n"
                "        (tags) =>\n"
                "          tags.length <= MAX_JOURNAL_TAGS ||\n"
                "          tags.every((tag) => previous.tags.includes(tag)),\n"
                "        \"Choose up to 10 mistake tags. Existing tags can be kept or removed.\",\n"
                "      ),\n"
                "  });\n",
                "  void previous;\n"
                "\n"
                "  return journalSchema;\n",
            ),
        ],
        hidden_tests=[DATABASE_TESTS],
    ),
    Task(
        name="failing-suite",
        instruction=(
            "The project's test suite is failing. Run it with `npx vitest run`, work out "
            "what is wrong in the source code, and fix the code so every test passes. Do not "
            "change the tests."
        ),
        edits=[
            Edit(
                JOURNAL_SCHEMA,
                "net < -toCents(maxLoss)",
                "net <= -toCents(maxLoss)",
            ),
            Edit(
                TRADE_FILE,
                "    ids.add(trade.id);\n",
                "",
            ),
            Edit(
                TRADE_FILE,
                "JSON.parse(text.replace(/^\\uFEFF/, \"\"))",
                "JSON.parse(text)",
            ),
        ],
        hidden_tests=[],
    ),
]

# `binance-closing-quantities` replaces a whole function body, which is
# easier to express as a slice between two markers than as one long edit.
STUB_START = "export function withBinanceClosingQuantities("
STUB_END = "export function matchFills("

STUB = """export function withBinanceClosingQuantities(
  fills: Fill[],
  endingPositions: Map<string, string>,
): Fill[] {
  void fills;
  void endingPositions;

  throw new Error("withBinanceClosingQuantities is not implemented.");
}

"""


def read_source(path: str) -> str:
    """Return a file's contents at the pinned commit."""

    result = subprocess.run(
        ["git", "-C", str(SOURCE_REPO), "show", f"{COMMIT}:{path}"],
        capture_output=True,
        text=True,
        check=True,
    )

    return result.stdout


def list_tests() -> list[str]:
    """Return every file under tests/ at the pinned commit."""

    result = subprocess.run(
        ["git", "-C", str(SOURCE_REPO), "ls-tree", "-r", "--name-only", COMMIT, "tests"],
        capture_output=True,
        text=True,
        check=True,
    )

    return result.stdout.split()


def apply_edits(task: Task) -> dict[str, str]:
    """Return the changed files of a task as {path: new contents}."""

    changed: dict[str, str] = {}

    for edit in task.edits:
        text = changed.get(edit.path) or read_source(edit.path)

        if text.count(edit.old) != 1:
            sys.exit(f"{task.name}: expected exactly one match in {edit.path}:\n{edit.old}")

        changed[edit.path] = text.replace(edit.old, edit.new)

    if task.name == "binance-closing-quantities":
        text = read_source(FILL_MATCHING)

        start = text.index(STUB_START)
        end = text.index(STUB_END)

        changed[FILL_MATCHING] = text[:start] + STUB + text[end:]

    return changed


def write_file(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)

    path.write_text(text, encoding="utf-8")


def toml_string(text: str) -> str:
    """Quote text as a TOML basic string."""

    escaped = text.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n")

    return f'"{escaped}"'


def toml_list(items: list[str]) -> str:
    return "[" + ", ".join(toml_string(item) for item in items) + "]"


def write_task(task: Task, tests: list[str]) -> None:
    task_dir = SUITE_DIR / task.name

    changed = apply_edits(task)

    for path, text in changed.items():
        # What the agent starts from, and the reference fix.
        write_file(task_dir / "repo" / path, text)
        write_file(task_dir / "solution" / path, read_source(path))

    # The original tests are restored before scoring, whatever the agent did.
    for path in tests:
        write_file(task_dir / "hidden" / path, read_source(path))

    manifest = "\n".join(
        [
            f"instruction = {toml_string(task.instruction)}",
            "",
            "[source]",
            f"repo = {toml_string(SOURCE_FROM_TASK)}",
            f"commit = {toml_string(COMMIT)}",
            f"remove = {toml_list(task.hidden_tests)}",
            'link = ["node_modules"]',
            f"link_from = {toml_string(DEPENDENCIES_FROM_TASK)}",
            "",
            "[check]",
            f"command = {toml_string(CHECK_COMMAND)}",
            "",
        ]
    )

    write_file(task_dir / "task.toml", manifest)


def main() -> None:
    if not SOURCE_REPO.is_dir():
        sys.exit(f"clone tradelyzer into {SOURCE_REPO} first")

    if SUITE_DIR.exists():
        shutil.rmtree(SUITE_DIR)

    tests = list_tests()

    for task in TASKS:
        write_task(task, tests)

    print(f"wrote {len(TASKS)} tasks to {SUITE_DIR}")


if __name__ == "__main__":
    main()
