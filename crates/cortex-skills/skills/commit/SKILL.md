---
name: commit
description: Commit the current work to git with a message in the repository's own style.
argument-hint: "[what to commit or a note for the message]"
---

Make one git commit of the work that is ready.

Arguments: $ARGUMENTS

1. Look first, in one step: `git status --short`, `git diff HEAD`, and `git log --oneline -10` for the style of earlier messages.
2. Decide what belongs in this commit. Leave out files that are unrelated, generated, or look like secrets (`.env`, keys, credentials, tokens); say so if you leave something out for that reason. If the changes are two separate pieces of work, say so and commit only the one asked for.
3. Stage by name: `git add <path> ...`. Do not use `git add -A` or `git add .`.
4. Write the message the way this repository writes them (prefixes, tense, length). A subject of at most about 70 characters that says what changed and why it matters, then, only if needed, a short body on the reason. Describe the change, not the conversation that led to it.
5. Commit with the message passed through a heredoc so that quoting cannot mangle it:

   ```
   git commit -F - <<'EOF'
   subject

   body
   EOF
   ```
6. Run `git status --short` to confirm, and report the commit's hash and subject.

Rules: never `--amend`, never `--no-verify`, never change git configuration, never push unless asked. If a hook fails, the commit did not happen: fix what it reports and make a new commit. If the repository is on its default branch and the person asked for a pull request, create a branch first (`git switch -c <name>`).
