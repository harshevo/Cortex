---
name: fix-ci
description: Find out why the GitHub checks on a pull request or branch are failing and fix the cause.
argument-hint: "[PR number]"
---

Get a pull request's failing checks to pass by fixing what they found.

Arguments: $ARGUMENTS

1. See which checks failed (`github` tool `pr_checks`, or `gh pr checks`). If all pass or are still running, report that and stop.
2. For each failed check, read the failing part of its log (`run_log` with the run's id, or `gh run view <id> --log-failed`). Find the first real error, not the last line: later failures are often consequences.
3. Decide what kind of failure it is:
   - Caused by the change (test, type, lint, build): continue.
   - Already failing on the base branch, flaky, or infrastructure (network, runner, missing secret): do not paper over it. Report it with the evidence.
4. Reproduce locally with the same command the workflow runs (read `.github/workflows/` for it), then fix the cause. Do not weaken the check: no skipped or deleted tests, no loosened assertions, no disabled lint rules, unless the test itself is what is wrong and you can say why.
5. Run the same command again until it passes, and the neighbouring checks you may have affected.
6. Report per check: cause, fix, and the local result. Do not commit or push unless asked; the checks only run again once the fix is pushed.
