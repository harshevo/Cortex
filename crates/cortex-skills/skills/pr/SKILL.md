---
name: pr
description: Open a GitHub pull request for the current branch, with a title and description written from all of its commits.
argument-hint: "[base branch] [--draft]"
---

Open a pull request for the work on the current branch.

Arguments: $ARGUMENTS

1. Look first, in one step: `git status --short`, `git branch --show-current`, and the repository's default branch (`github` tool `repo`, or `gh repo view --json defaultBranchRef`).
2. If there is uncommitted work that belongs to this change, commit it as the `commit` skill describes. If the current branch is the default branch, create a branch for the work first (`git switch -c <short-descriptive-name>`); do not push to the default branch.
3. Read everything the pull request will contain, not only the last commit: `git log --oneline <base>..HEAD` and `git diff <base>...HEAD --stat`, then the diff itself where the summary is not enough.
4. Check whether a pull request for this branch already exists (`pr_view` with no number). If it does, report its address instead of opening another.
5. Push: `git push -u origin <branch>`. Never force-push.
6. Open it (`github_write` with `pr_create`, or `gh pr create --title ... --body-file -`). Add `draft` when the arguments say `--draft`.
   - Title: under about 70 characters, what the change does.
   - Body:

     ```
     ## Summary
     - what changed and why, 1 to 3 points

     ## Test plan
     - what you ran and its result
     - what a reviewer should check by hand, if anything
     ```

     Link an issue with `Fixes #N` only when this change resolves it.
7. Report the pull request's address.

Do not merge, and do not enable auto-merge.
