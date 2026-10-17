---
name: resolve-conflicts
description: Resolve git merge or rebase conflicts, keeping the intent of both sides, and prove the result with the project's checks.
argument-hint: "[branch or note]"
---

Resolve the conflicts of the merge or rebase in progress.

Arguments: $ARGUMENTS

1. Find out where things stand, in one step: `git status`, and `git log --oneline -5` on each side (`HEAD` and `MERGE_HEAD`, or `REBASE_HEAD` during a rebase). If nothing is in progress and a branch was named, merge it first with `git merge --no-ff <branch>`, unless the person asked for a rebase.
2. For every conflicted file, read the whole conflict, and what each side changed and why: `git log --oneline -3 -- <file>` on each side, and `git diff <base> <side> -- <file>` for each side, where `<base>` is `git merge-base HEAD MERGE_HEAD`.
3. Write the resolution that keeps the intent of both sides. Never pick one side wholesale unless the other side's change is truly superseded; say so when you do. Keep the code compiling between files: a renamed function on one side and a new call on the other means the call takes the new name.
4. Check that no markers are left: search for `<<<<<<<`, `=======` and `>>>>>>>` in the files you resolved.
5. Run the project's build and tests. Fix what the merge broke. A resolution that has not been run is not done.
6. Stage the resolved files by name and finish with `git commit --no-edit` (merge) or `git rebase --continue` (rebase), then report each file and how it was resolved, and the checks you ran.

Rules: never `git checkout --theirs`/`--ours` on a whole file without reading both sides; never `git reset --hard`, `git merge --abort` or `git rebase --abort` unless the person asks; never push.
