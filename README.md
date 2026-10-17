# Cortex

Cortex is a Rust coding harness with a native desktop UI and a command-line agent.
It supports provider-backed chats, project files, tool approvals, code search,
verification, skills, session history, split chats, and collaboration between chats.

## Run

```sh
cargo run -p cortex-gui
cargo run -p cortex-cli -- chat --dir /path/to/project
```

Choose a provider and model in the desktop Settings, or use the CLI provider flags.
Provider credentials and desktop settings are kept under `~/.cortex`; project chat
logs are kept under `.cortex/trajectories`. No neural learner or brain service is started.

```sh
cargo run -p cortex-cli -- --help
cargo run -p cortex-cli -- run --help
```

## Commands and checkpoints

Before a run first changes anything, the project is saved as a checkpoint in
`.cortex/checkpoints` (a git store of its own; the project's repository is not
touched). `.gitignore`d files, `target/` and `node_modules/` are left out.

Type `/` in the app's composer for the command list:

| Command | Does |
| --- | --- |
| `/undo` | Revert the last run's changes |
| `/checkpoints`, `/restore <id>` | List checkpoints, go back to one |
| `/diff` | Files changed since the last checkpoint |
| `/notes` | What Cortex remembers about the project |
| `/run`, `/learned`, `/trace` | Run panel, what was learned, last run's summary |
| `/worktree <name>` | New branch `cortex/<name>` in its own folder, opened as a project |
| `/tasks` | Show or hide the task list the agent keeps with `update_plan` |
| `/new`, `/compact`, `/prepare`, `/stop`, `/settings`, `/help` | Session controls |

Skills (`/code-review`, …) are listed alongside. From the terminal:
`cortex undo`, `cortex checkpoints [id]`. `--no-checkpoints` turns them off.

Destructive calls (`rm -rf`, `git reset --hard`, force pushes, `sudo`,
`delete_file`, …) are always asked about, even with "approve everything" on.

## Models, memory and extensions

- **Fallback and helper models.** `--fallback-model <name>` (repeatable) is
  tried in order when the model in use is rate limited or down; a failing model
  rests for two minutes. `--helper-model <name>` does reviews, reflection and
  compaction. Both are under Settings → Models in the app.
- **Notes.** The agent keeps what it learns with `save_note` / `drop_note` in
  `.cortex/notes.json`; relevant notes, and similar earlier runs from
  `.cortex/episodes.jsonl`, are put before each task. A note whose files have
  changed since is marked as possibly out of date.
- **Rules.** `AGENTS.md`, `CLAUDE.md`, `.claude/CLAUDE.md`, `CORTEX.md`,
  `.cortex/rules.md` and `~/.cortex/rules.md` are added to the system prompt.
- **Hooks.** `.cortex/hooks.json` in the project or home, in Claude Code's
  format (`PreToolUse`, `PostToolUse`). Exit code 2 blocks the call (pre) or
  shows the hook's output to the model (post). Hooks run in the sandbox.
- **MCP.** Servers in `~/.cortex/mcp.json` (`{"mcpServers": {...}}`, stdio)
  add their tools as `mcp__server__tool`. Only the home file is read, so
  opening a repository never starts a program. `--no-mcp` turns them off.
- **Skills.** Includes `resolve-conflicts`, alongside `commit`, `pr`,
  `review` and `fix-issue`.

## Headless

`cortex run --json` writes every event as a JSON line, then a final
`{"type":"result", ...}` line. It denies anything that needs approval unless
`--yes` is given. Exit code 0 means the run completed, 3 means it completed but
the checks failed, and 2 means anything else (step limit, stuck, over budget).

## Recovery, budgets, replay and learning

- **Self-recovery.** The stuck detector notices the same failing call
  three times, near-identical patches, edits that undo earlier ones, and
  steps where every call fails. It says what the failures look like
  (environment, tool call, or the work itself). If the run keeps failing
  after that, it moves to `--escalation-model` (Settings → Models), and
  later it stops with the outcome `stuck`. `/undo` reverts it.
- **Budgets.** `--max-cost <usd>`, `--max-tokens <n>` and
  `--max-minutes <m>` (Settings → Permissions). At 80% the run is told to
  wrap up; at 100% it stops with `over_budget`.
- **Replay and debugging.** Logs record approvals, escalations and budget
  events, and secrets are redacted before anything is written.
  `cortex trace <log>` summarises a run, `--steps` lists its calls,
  `--compare <other.jsonl>` shows where two runs parted ways, and `--replay`
  makes the calls again in a copy of the project and shows the first output
  that differs.
- **Impact analysis.** The `impact` tool (with the code graph on) lists, for
  files or a symbol: what depends on them two levels deep, the tests that
  reach them, their recent commits, the notes about them, and how earlier
  runs that changed them went.
- **Learning.** Each run is kept with its tools, tokens and the notes it was
  shown. Before a task Cortex is told how similar tasks went and which tools
  the passing runs used. That estimate also sizes the run panel. Notes never
  shown in a passing run are flagged. A tool sequence shared by three
  passing runs is offered as a skill. `cortex learned [--accept|--dismiss
  <name>]` covers the same from the terminal.

In the app, **⌘I** (or `/run`, `/learned`) opens the panel on the right:
**Run** shows steps against the estimate, budget, recovery, approvals and
changed files; **Learned** shows playbooks, doubtful notes and skills to keep.
`/trace` summarises the last run in the conversation.

## Workspace

| Crate | Purpose |
| --- | --- |
| cortex-core | Agent loop, tools, approvals, context, logs, and verification interfaces |
| cortex-providers | Provider adapters and authentication |
| cortex-index | Code index and dependency graph |
| cortex-retrieval | Optional semantic code search |
| cortex-router | Project maps and task context routing |
| cortex-tools | File, shell, search, browser, and GitHub tools |
| cortex-skills | Written procedures for the agent |
| cortex-verify | Project check discovery and execution |
| cortex-eval | Harness evaluation runner and reports |
| cortex-cli | Command-line interface and shared harness setup |
| cortex-gui | Native desktop UI |

## Optional semantic search

Code search by meaning uses a separate embedding service. Ordinary chats and
file/search tools work without it.

```sh
uv sync --project ml
uv run --project ml python -m cortex_ml.embedding.server
cargo run -p cortex-cli -- search --help
```

The service uses `google/embeddinggemma-2`; model access may require Hugging Face
authentication. See `ml/README.md`.

## Verification

```sh
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release -p cortex-cli -p cortex-gui
```

`scripts/package.sh` packages macOS/Linux builds; `scripts/package.ps1` packages
Windows builds. Packages include the optional embedding runtime. Run packaging
on the destination operating system and architecture.
