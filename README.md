# Cortex

An experimental coding agent for the terminal. It takes an instruction, works in your project with tools (read, search, edit, run commands), checks its own work, and reports back.

The research goal is to measure whether small specialised components (learned memory, a code world model, a router) make a frontier model better at real coding tasks. The full plan is in [`../plan.md`](../plan.md). So far it has the plain agent loop, the evaluation harness, and three optional capabilities that are being measured against the plain loop: a code index, semantic search, and context pruning.

## Setup

```bash
cargo build --release
```

```bash
export ANTHROPIC_API_KEY="..."
```

## Use

Carry out one instruction in the current directory:

```bash
cargo run --release -- run "fix the failing tests"
```

Start an interactive session:

```bash
cargo run --release -- chat
```

Cortex asks before it edits a file or runs a command. Answer `a` to allow everything for the session, or pass `--yes` to skip the questions. Other options: `--dir`, `--model`, `--effort`, `--max-steps`.

Every run is logged to `.cortex/trajectories/` inside the project.

### Desktop app

A native window onto the same agent, drawn on the GPU:

```bash
cargo run --release -p cortex-gui
```

The desktop app starts the local embedding service automatically and waits for its model to be ready. All chat agents share the same service, and a healthy service already running on port 8765 is reused. It uses `ml/.venv/bin/python` when available; otherwise `uv` sets up the Python environment. The first launch may download the model weights. Startup errors are recorded in `~/.cortex/logs/embedding-service.log`. If the source directory was moved, set `CORTEX_ML_DIR` to its `ml` directory. Demo mode remains offline and does not start the model.

Set it up on its **Settings** page: the provider, its API key (kept in the system keychain, not in a file), the models to offer, permissions, and the same optional capabilities `--with` switches on. The other choices are saved in `~/.cortex/app.json`. Arguments that work for `cortex run` (`--provider`, `--model`, `--base-url`, `--with`, `--dir`, `--yes`) are accepted too and are taken over into the settings.

The sidebar lists project folders, each with its own sessions, and below them the sessions that belong to no project (these work in `~/.cortex/sessions`). Under the message box are the speed of the latest model call, the share of input served from the provider's cache, the loop the agent is running, and selectors for the model, the reasoning effort (Anthropic only) and whether to ask before changes.

Use the **...** menu beside the view controls (or **⌘N**) to start another independent agent. The menu switches between open chats, closes the current chat, and selects **Single pane**, **Side by side**, or **Top and bottom**. Chats keep running when you switch. Split panes have separate composers, approvals, model choices and stop buttons; drag the divider to resize them. Click inside a pane to make its project and session active in the sidebar. Saved sessions remain in the sidebar. `cargo run -p cortex-gui -- --split` starts with two panes.

Agents in the same project can collaborate through `team_list_agents`, `team_send_message` and `team_read_messages`. For example, ask one chat to implement an API and another to review its tests, and tell them to coordinate file ownership with their peers. Messages reach a working agent between steps and start a turn for an idle agent. Each agent keeps its own conversation and trajectory; peers share project files and memory, so they should agree on which files each will edit. Agents in different projects cannot send messages to each other.

A saved session can be continued by clicking it in the sidebar. If its chat is already open, that chat is selected; otherwise a new independent worker resumes its conversation. Search filters projects and saved sessions. During a run, new messages are queued until the model reads them, and stopping returns unread messages to the composer. Provider reasoning streams live and can be hidden; it is not stored in the log. Settings are grouped into Models & accounts, Permissions and Capabilities. Settings applied during a run take effect when that agent finishes.

**Memory space** draws the agent's memory as a brain: working memory for the current run at the front, episodes in the middle, long-term facts over the cortex, and the best-connected neurons of the intuition (`--with intuition`) in the cerebellum with their synapses. Memory is always on in the app, in the same `.cortex/memory.db` the command line uses. New desktop sessions also enable Brain, Intuition, and Gate by default; Capabilities can switch them off or choose the baseline controller. Earlier memory-only desktop settings migrate once to these defaults.

The fly network is now connected to session chat events. A separate **Fly network** region shows up to 96 real sampled neurons and their signed synapses. The **Brain inspector** plays back the 24 recorded simulation ticks, with actual post-reset voltage, spike indicators, input features, action values, training counts, and simulation time. Playback is slowed for inspection, not a continuously firing biological brain. Any neuron ID in the full network can be inspected, with its outgoing connections paginated in groups of 64. Positions are display layouts, not measured anatomical coordinates. The model's internal neurons are not exposed.

Intuition activity and learned connections update from the running chat. Opening an old chat restores its recorded brain activity and working memory. Persistent memory includes chat-only answers and the last five successful calculation results per task. `calculate` evaluates numeric arithmetic locally using bounded double-precision parsing; it does not execute code. `brain_state` exposes bounded diagnostics when a brain adviser is enabled. Raw simulation frames are logged as `brain_observed` events, never inserted automatically into the model prompt.

Parallel chats merge learned state under file locks and replace JSON atomically. SQLite consolidates episodes and derived facts in one transaction. Default generated fly wiring is shared between desktop chats; voltages and working state remain independent. The inspector reports observed verification outcomes and median task steps. These observations are not a controlled accuracy comparison; use the evaluation harness to compare enabled capabilities against the baseline.

`--demo` runs a scripted stand-in model on a throwaway project, with no network and no key, and saves nothing. Its tools, memory and checks are real.

### Other model providers

`--provider local` talks to an OpenAI-compatible server on this machine, such as Ollama. `--provider hosted --base-url <url> --model <id>` talks to any hosted OpenAI-compatible service, reading the key from `CORTEX_API_KEY`.

`--provider codex` uses the models of a ChatGPT subscription, through the service the Codex command-line tool talks to. Only the model is used: every request carries Cortex's own instructions, conversation and tools, and the loop stays Cortex's. Sign in once with `cortex login`, which opens the browser and saves the tokens in `~/.cortex/codex-auth.json` (readable by you only); they are renewed as needed. Without that file, the sign-in the Codex tool keeps in `~/.codex/auth.json` is used as it is, and never renewed or rewritten. `--model` defaults to `gpt-6.1-sol`, and `--effort` is passed on (`max` as `xhigh`). Usage counts against the plan's Codex allowance, so no cost is shown.

### Optional capabilities

CLI capabilities are off by default and turned on with `--with`, so their effects can be measured independently. Desktop defaults are described above.

| Flag | What it adds |
|---|---|
| `--with index` | `outline` and `read_symbol`: see a file's functions and read one definition by name, instead of whole files |
| `--with semantic` | `semantic_search`: find code from a description of its behaviour, using EmbeddingGemma 2. Needs the service in [`ml/`](ml/README.md) running |
| `--with prune` | Remove tool output that a later, identical call has replaced (a file read again after an edit, a test command run again). Nothing else is removed, however old. Not usable with the Anthropic API |
| `--with map` | Show the model a map of the project before it starts: every source file with its line count and top-level names, at most about 1,500 tokens |
| `--with lean` | Return less text from the basic tools: an outline for a whole source file over 400 lines, the changed lines after an edit, only the summary of a passing test, build or lint run, and one line for a repeated read that has not changed |
| `--with batch` | Ask the model to put independent lookups in one response. Read-only calls in one response run at the same time whether or not this is on |
| `--with graph` | `find_references`, `file_dependencies` and `affected_tests`: every call site of a name, followed through imports, aliases and re-exports; which files import which; and which tests import the files a change touches |
| `--with memory` | Memory in three tiers (below). What earlier tasks in this repository changed, ran and got wrong is shown before each new task. Adds the `remember` and `recall` tools |
| `--with gate` | The harness runs the project's own checks when the model says it is done, and sends failures back instead of accepting the claim. `--check "<command>"` overrides the detected checks |
| `--with router` | Before the model starts, gather the context this kind of task needs from whichever of the above are on: memory, call sites of names the task mentions, code found by meaning, or the output of failing checks |
| `--with controller` | A decision controller. From runs whose checks passed or failed it learns which next action (inspect, edit, run, revert, finish) pays off in which situation, and says so in one line between steps once it is confident. Learns only together with `--with gate`. This is the plain baseline |
| `--with brain` | The same controller, with the situation first passed through a spiking network the size of the fruit fly brain: 139,255 neurons, about 50 million synapses, 300 MB. The wiring is generated unless `--connectome <file>` supplies the measured one: the FlyWire connectome, 138,639 neurons and 54.5 million synapses, 91 MB (see [`ml/`](ml/README.md) for how to get it) |
| `--with intuition` | A second brain beside the controller: an associative network with a neuron for each task word, file, command and error it has met, and synapses strengthened by passing runs, strengthened at half rate by completed unchecked runs, and weakened by failures. Before a task it names the files that tasks worded like it went on to change; between steps, what passing runs went on to from the file just read or the error just met; on a long run, a check left failing or edits nothing has checked. Silent until it has learned from four runs. Works with or without `--with brain` |
| `--with hope` | Let the HOPE-style memory model re-score recalled memories. Needs `--with memory` and the service in [`ml/`](ml/README.md). Untrained, it ranks exactly like plain similarity |

### Memory

| Tier | Holds | Lives for |
|---|---|---|
| Working | What this run has read, changed, run and got wrong. Built from the run's events, at no cost to the model | One task |
| Episodic | One record per finished task: files changed, the files read on the way, commands that worked, dead ends, whether the checks passed, the model's report | Until deleted |
| Long-term | Facts about the repository: the check command that passed, files several tasks changed, commands that keep failing, notes the agent saved with `remember` | Until the file they mention is gone |

After each task the working memory is consolidated into an episode, and patterns seen in several episodes are promoted to long-term facts. Memory is kept in `.cortex/memory.db` (SQLite). Tasks are compared with EmbeddingGemma 2 when the embedding service is running and by shared words when it is not.

See what the agent would be shown for a task, without calling a model:

```bash
cargo run --release -- context "fix the calendar's last week" --with memory --with graph --with router --with semantic
```

## Evaluate

Check that the task suite itself is sound (no model or API key needed):

```bash
cargo run --release -- eval validate
```

Run the agent on the suite and save a report:

```bash
cargo run --release -- eval run --label baseline --repeats 3
```

Compare two reports to see whether a change helped:

```bash
cargo run --release -- eval compare eval/results/<baseline>/report.json eval/results/<candidate>/report.json
```

Model output varies from run to run, so use at least three repeats before trusting a comparison.

## Layout

```
crates/
  cortex-core/        agent loop, conversation types, and the traits below
  cortex-providers/   model backends (Anthropic API, OpenAI-compatible, ChatGPT subscription; a scripted one for tests)
  cortex-index/       Tree-sitter symbols and the code graph (Python, TypeScript, JavaScript, Rust)
  cortex-retrieval/   semantic search: chunking, embedding client, ranking
  cortex-memory/      working, episodic and long-term memory; consolidation; recall
  cortex-verify/      the verification gate: finds and runs a project's checks
  cortex-router/      picks and gathers context for a task before the model starts
  cortex-brain/       decision controllers (a fly-scale spiking network and a plain baseline) and the intuition network
  cortex-tools/       the tools the agent can call
  cortex-eval/        task suites, scoring, reports, comparison
  cortex-cli/         the `cortex` command
ml/                   Python: the embedding service, the HOPE-style memory model, the edit predictor
eval/
  suites/smoke/       five small Python tasks
  suites/tradelyzer/  twelve defects planted in a real TypeScript project
  tools/              scripts that build suites and measure retrieval
  sources/            cloned repositories the suites are built from (not committed)
  results/            reports from eval runs (not committed)
```

Each later phase plugs into one of seven traits in `cortex-core`:

| Trait | Purpose | Implemented by |
|---|---|---|
| `ModelProvider` | Talk to a model | `cortex-providers` |
| `Tool` | Something the agent can do | `cortex-tools` |
| `TrajectorySink` | Receive every event of a run | the log file, the console |
| `Approver` | Decide whether a change may run | the terminal prompt, auto-approve |
| `ContextSource` | Put context in front of an instruction | memory, the router |
| `Verifier` | Check finished work | `cortex-verify` |
| `Adviser` | Add a note between steps | the decision controllers and the intuition in `cortex-brain` |

## Adding an eval task

Create a directory under a suite with:

- `task.toml`: the `instruction` and a `[check]` `command` that exits 0 when the task is solved.
- `repo/`: the project the agent starts from.
- `hidden/`: files copied in after the agent finishes, normally the scoring tests. The agent never sees them.
- `solution/`: a reference fix, used only by `eval validate`.

Then run `eval validate`. A task must fail untouched and pass with its solution.

## Known limits

- Commands run directly on your machine with no sandbox. File tools are confined to the project directory; the `shell` tool is not.
- The code graph reads syntax, not types. Uses reached through an import are exact; a method call on a value (`order.total()`) cannot be tied to one class and is listed as a possible match. Rust imports across crates are not resolved.
- Memory is consolidated by counting, not by a model. It records which files a task touched, not why.
- The verification gate is only as strong as the project's own checks. A task whose bug no existing test covers passes the gate unverified in effect.
- The router decides with hand-written rules; nothing is learned yet.
- The fly-scale brain has the fly's size and sparsity and a mushroom-body-style learning circuit, but generated wiring. With a measured connectome it uses the fly's real wiring, which evolved for flying and feeding; whether either helps a coding agent is untested. `--with controller` is the baseline it has to beat.
- The controllers stay silent until they have learned from five checked runs, and learn nothing without `--with gate`. Replayed over 40 saved runs (`cargo run --release -p cortex-brain --example replay`), the fly-scale brain predicted no better than the 17-number baseline, and neither much better than a constant.
- The intuition recalls what earlier runs found; it finds nothing new. Replayed over 24 saved tradelyzer runs (`--example intuition_replay`), it named a file before the task in 6 runs, all 6 correctly, and stayed silent on tasks it had never met. Those runs opened that file after 1.3 steps anyway, so on short tasks it saves about a step. Its effect on a model, and on long tasks, is unmeasured.
- Responses are not streamed, so text appears a message at a time.
- The smoke suite is small and easy. It proves the harness works; it is not a benchmark.
