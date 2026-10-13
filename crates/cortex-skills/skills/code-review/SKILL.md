---
name: code-review
description: Review local changes (the working tree, a branch, or given paths) for real bugs, and report verified findings ranked by severity.
argument-hint: "[path | branch | commit range] [--fix]"
---

Review code changes for defects that would matter to the people who run this code. This is a review: do not edit anything unless the arguments contain `--fix`.

Arguments: $ARGUMENTS

## 1. Decide what is under review

- No arguments: the uncommitted work. Run `git status --short` and `git diff HEAD`. Read untracked files that are part of the change in full. If the tree is clean, review the current branch against the default branch instead: `git diff $(git merge-base HEAD origin/HEAD)...HEAD` (fall back to `main` or `master` if `origin/HEAD` is not set).
- A branch or commit range: `git diff <merge-base>...<branch>` or the range as given.
- A path: the changes under that path, or the files themselves if they have no changes.
- A pull request number or URL: use the `review` skill instead.

Start with `git diff --stat` for the same range, so you know the size before reading. For a large change, read it file by file rather than in one piece.

## 2. Understand before judging

For every changed hunk, read enough of the surrounding file to know what the code is for: the whole function, its callers when a signature or behaviour changed (search for the name), and the tests that cover it. A diff read without its context produces false findings.

## 3. Look for defects

In order of what matters:

1. Wrong results: inverted or off-by-one conditions, wrong variable, missed case, broken invariant, a changed function whose callers were not updated.
2. Failures at the edges: empty, missing, zero, negative, very large, duplicate, non-ASCII input; error paths that swallow, mislabel or leak; resources not released.
3. Concurrency and ordering: shared state without protection, await points that let state change, retries that repeat a side effect.
4. Data and compatibility: migrations, stored formats, public interfaces and configuration that old data or old callers still depend on.
5. Security: untrusted input reaching a query, a shell, a path, a template or a deserializer; missing authorization checks; secrets in code or logs.
6. Tests: behaviour that changed with no test that would fail if it broke again; tests that pass without exercising the change.

Then, less important, and only when clear: code that duplicates something the project already has, and work done in a loop that could be done once.

Do not report formatting, naming taste, missing comments, or anything a linter or the compiler already reports. Do not report problems in code the change did not touch unless the change makes them reachable.

## 4. Verify each candidate

Before a finding goes in the report, try to prove it wrong. Trace the exact input and state that triggers it through the code as written. Where it is cheap, run it: an existing test, a one-line script, the type checker. Drop what you cannot make concrete; mark as "unconfirmed" only what is serious enough to mention anyway.

## 5. Report

Findings first, most severe first. For each:

- `path:line` and a one-sentence statement of the defect.
- The concrete case: what input or state, and what goes wrong.
- The fix, in a sentence or a few lines of code.

Then one line on what you checked and how (tests run, files read), and anything you could not look at. If you found nothing, say so plainly; do not pad the report with minor remarks.

With `--fix`: after reporting, apply the fixes for the confirmed findings, smallest change each, run the relevant tests, and say what changed.
