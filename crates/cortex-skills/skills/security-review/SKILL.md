---
name: security-review
description: Security review of the pending changes on the current branch (or given paths): find exploitable vulnerabilities by tracing untrusted input to where it is used.
argument-hint: "[path | branch | PR number]"
---

Find vulnerabilities that this change introduces and that someone could actually exploit. Report only; do not edit.

Arguments: $ARGUMENTS

## 1. Scope

Default: everything on the current branch that is not on the default branch, plus uncommitted work: `git diff $(git merge-base HEAD origin/HEAD)` (fall back to `main` or `master`), and `git status --short` for new files. A path narrows it; a pull request number means that pull request's diff (`github` tool `pr_diff`, or `gh pr diff`).

## 2. Learn how the project defends itself

Before judging the change, find what the project already does: how requests are authenticated and authorized, how input is validated, how queries are built (ORM, parameters), how output is escaped, where secrets come from. A change is suspicious where it departs from these habits.

## 3. Trace

For each place the change handles data from outside (request fields, headers, files, environment, command-line arguments, database rows written by users, responses from other services, model output), follow it to where it is used. A finding needs both ends: a source an attacker controls and a sink where that matters, with no effective check between.

Sinks to follow input into:

- Injection: SQL and other queries built from strings; shell commands; `eval` and dynamic imports; templates; LDAP, XPath, regular expressions built from input.
- Files and network: paths joined from input (traversal), archive extraction, redirects, requests to addresses from input (SSRF).
- Deserialization of untrusted data (pickle, YAML load, Java or .NET object streams).
- HTML output without escaping (XSS), `dangerouslySetInnerHTML`, `innerHTML`.

Also check, independent of data flow:

- Authentication and authorization: a new route or action without the check its neighbours have; an object fetched by id without confirming it belongs to the caller; privilege decided from a value the client sends.
- Secrets and cryptography: keys, tokens or passwords in code, logs or error messages; home-made crypto; weak hashes for passwords; predictable randomness for tokens; disabled certificate checks.
- Sessions and tokens: missing expiry, tokens in URLs, cookies without the flags the project uses elsewhere, CSRF protection removed or skipped.
- Sensitive data exposed in responses, logs or analytics.
- New dependencies: known-vulnerable versions, typo-squatted names, install scripts.

## 4. Keep only what is real

For each candidate, write down the attack: who the attacker is, what they send, what they get. If you cannot, drop it. Read the code on the path to confirm that no validation, framework default or type already stops it.

Do not report: denial of service by volume, missing rate limits, theoretical timing attacks, issues only reachable by someone who already has full access, problems in test files, outdated dependencies with no reachable vulnerable code, or general hardening advice. Do not report what the change did not introduce or expose.

## 5. Report

For each finding, most severe first:

- `path:line`, category, severity (high: direct compromise of data, accounts or the host; medium: needs a specific condition; low: defence in depth), and your confidence.
- The attack in two or three sentences.
- The fix, using the project's own mechanism where one exists.

End with what you covered and what you could not see (for example configuration that lives outside the repository). If nothing is exploitable, say that in one line.
