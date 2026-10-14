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

The commands the agent runs (`shell`, the gate's checks, `gh`) go through a sandbox unless `--no-sandbox` is given, or the switch under **Settings → Permissions** in the app is turned off. Inside it a command can write only in the project, temporary folders and the caches build tools keep, cannot read where keys are kept (`~/.ssh`, `~/.aws`, shell start-up files and the like), and is not handed `ANTHROPIC_API_KEY` or `CORTEX_API_KEY`. The network stays open. It is macOS only so far (Seatbelt); elsewhere commands are not confined and a line says so. The browser is not inside it.

Every run is logged to `.cortex/trajectories/` inside the project.

### Skills

A skill is a written procedure for one kind of task. Start one by typing `/name` as the instruction, in `run`, `chat` or the desktop app's composer; no flag is needed:

```bash
cargo run --release -- run "/code-review"
```

| Skill | What it does |
|---|---|
| `/code-review [path or branch] [--fix]` | Review uncommitted work, a branch or a path for real bugs; each finding is checked against the code before it is reported |
| `/review [PR] [--comment]` | Review a GitHub pull request; posts to it only with `--comment` |
| `/security-review [path, branch or PR]` | Trace untrusted input to where it is used and report what could be exploited |
| `/ui-design [what]` | Design or restyle an interface: follow the project's existing system, hierarchy, type, spacing, states, accessibility |
| `/browser [address or file, and what to do]` | Open a page in a real browser to look at an interface, test a flow the way a person would use it, or carry out a task on a page. Needs `--with browser` |
| `/commit` | One commit in the repository's own message style, staged by name |
| `/pr [base] [--draft]` | Push the branch and open a pull request written from all of its commits |
| `/pr-comments [PR] [--reply]` | Address the review comments on a pull request |
| `/fix-issue <n> [--pr]` | Read an issue, reproduce it, fix it with a test |
| `/fix-ci [PR]` | Read the failing checks' logs and fix the cause |

`cortex skills` lists them, and `cortex skills <name>` prints one. Add your own as `~/.cortex/skills/<name>/SKILL.md` or, for one project, `.cortex/skills/<name>/SKILL.md` (a plain `<name>.md` works too); one with the name of a built-in skill replaces it. The file is Markdown with a `name` and `description` header, and `$ARGUMENTS` in it stands for what was typed after the name.

With `--with skills` the model is also shown the list and picks a skill itself when a task fits. With `--with github` it gets tools for GitHub, which the GitHub skills use when present and otherwise replace with `gh` in the shell. Both need the [`gh`](https://cli.github.com) command, signed in. With `--with browser` it gets a real browser, which the `/browser` skill is the procedure for.

### Desktop app

A native window onto the same agent, drawn on the GPU:

```bash
cargo run --release -p cortex-gui
```

The desktop app starts the local embedding service automatically and waits for its model to be ready. All chat agents share the same service, and a healthy service already running on port 8765 is reused. It uses `ml/.venv/bin/python` when available; otherwise `uv` sets up the Python environment. The first launch may download the model weights. Startup errors are recorded in `~/.cortex/logs/embedding-service.log`. If the source directory was moved, set `CORTEX_ML_DIR` to its `ml` directory. Demo mode remains offline and does not start the model.

**Dendritic network** (`--with brain` to let it advise, `--with living` to only watch; crate `cortex-living`): a network that grows and rewires itself as it watches runs. It creates a neuron for each new thing it meets and for things that keep happening together. Each neuron grows up to eight dendritic branches; a branch holds synapses from a few neurons that fired together one or two moments before this one did, and is active only when most of *its own* synapses are driven, so "an edit to this file, then the tests" and "the tests alone" are two branches with two records. From that it expects what comes next (each expectation as sure as its branch has been right before), imagines a few steps on by running on its own expectations, compares the actions it expects by what their imagined paths are worth, and can follow branches backward from an outcome to the steps that led to it. It keeps neurons for its own states ("I was surprised", "my advice was wrong"), predicts its own surprise, stays silent where it has learned its advice fails, replays recent moments when idle, and sets its own plasticity: each branch stiffens as it proves itself and loosens when it starts failing, and the whole network learns faster while more surprised than usual. With `--with brain` one line of this reaches the model between steps, only when the branch behind it has been tried at least four times and is as sure as the network currently requires of itself; the model can also ask it through the `brain_state` tool (`imagine`, `goal`). One network for all projects in `~/.cortex/living/network.json`; **Settings → Workspace → Open living network** draws it live, with its meters. Measured without a model on 83 saved runs (`cargo run --release -p cortex-living --example replay -- eval/results`, second half scored): it expects 46% of each moment and 56% of what it expects comes, against 24% and 30% for guessing the commonest percepts; the point-neuron network it replaced scored 59% and 39%. Whether its line helps a model is not measured. This is chaining over learned sequences, not reasoning in language, and the meters measure the mechanisms named: none of it is evidence of experience or awareness. 

**Settings → Workspace → Share memory between projects** makes the agent also recall the closest tasks and general lessons from your other projects (`--memory-from <FOLDER>` on the command line, with `--with memory`), and draws all of them in the memory space. A project's facts about its own files are never shared.

When a run ends, the mind's guess at what you will ask for next shows in the empty composer; **Tab** puts it in the box to send or edit. It comes from what you asked after earlier runs that ended the same way, and before there are any, from how the run ended (failing checks, or edits nothing checked). Needs the `mind` capability.

Set it up on its **Settings** page: the provider, its API key (kept in the system keychain, not in a file), the models to offer, permissions, and the same optional capabilities `--with` switches on. The other choices are saved in `~/.cortex/app.json`. Arguments that work for `cortex run` (`--provider`, `--model`, `--base-url`, `--with`, `--dir`, `--yes`) are accepted too and are taken over into the settings.

The sidebar lists project folders, each with its own sessions, and below them the sessions that belong to no project (these work in `~/.cortex/sessions`). Under the message box are the speed of the latest model call, the share of input served from the provider's cache, the loop the agent is running, and selectors for the model, the reasoning effort (Anthropic only) and whether to ask before changes.

Use the **...** menu beside the view controls (or **⌘N**) to start another independent agent. The menu switches between open chats, closes the current chat, and selects **Single pane**, **Side by side**, or **Top and bottom**. Chats keep running when you switch. Split panes have separate composers, approvals, model choices and stop buttons; drag the divider to resize them. Click inside a pane to make its project and session active in the sidebar. Saved sessions remain in the sidebar. `cargo run -p cortex-gui -- --split` starts with two panes.

Agents in the same project can collaborate through `team_list_agents`, `team_send_message` and `team_read_messages`. For example, ask one chat to implement an API and another to review its tests, and tell them to coordinate file ownership with their peers. Messages reach a working agent between steps and start a turn for an idle agent. Each agent keeps its own conversation and trajectory; peers share project files and memory, so they should agree on which files each will edit. Agents in different projects cannot send messages to each other.

A saved session can be continued by clicking it in the sidebar. If its chat is already open, that chat is selected; otherwise a new independent worker resumes its conversation. Search filters projects and saved sessions. During a run, new messages are queued until the model reads them, and stopping returns unread messages to the composer. Provider reasoning streams live and can be hidden; it is not stored in the log. Settings are grouped into Models & accounts, Permissions and Capabilities. Settings applied during a run take effect when that agent finishes.

**Memory space** draws the agent's memory as a brain: working memory for the current run at the front, episodes in the middle, long-term facts over the cortex, and the best-connected neurons of the intuition (`--with intuition`) in the cerebellum with their synapses. Memory is always on in the app, in the same `.cortex/memory.db` the command line uses. New desktop sessions also enable Brain, Intuition, and Gate by default; Capabilities can switch them off or choose the baseline controller. Earlier memory-only desktop settings migrate once to these defaults.

The dendritic network is drawn in the memory space as its own region: up to 320 of its most lately fired neurons and the strongest synapses between them, glowing as they fire. The **Brain inspector** shows what it expects next and why (the branch, and how often that branch has been right), what it imagines from here, how the best outcome it knows has been reached before, the last thing it said, and the rates it has set for itself. Positions are display layouts, not anatomy. The model's internal neurons are not exposed. 

Intuition activity and learned connections update from the running chat. Opening an old chat restores its recorded brain activity and working memory. Persistent memory includes chat-only answers and the last five successful calculation results per task. `calculate` evaluates numeric arithmetic locally using bounded double-precision parsing; it does not execute code. `brain_state` exposes bounded diagnostics when a brain adviser is enabled. Raw simulation frames are logged as `brain_observed` events, never inserted automatically into the model prompt.

Parallel chats merge learned state under file locks and replace JSON atomically. SQLite consolidates episodes and derived facts in one transaction.  The dendritic network is one per machine and shared between chats. The inspector reports observed verification outcomes and median task steps. These observations are not a controlled accuracy comparison; use the evaluation harness to compare enabled capabilities against the baseline.

`--demo` runs a scripted stand-in model on a throwaway project, with no network and no key, and saves nothing. Its tools, memory and checks are real.

### Phone apps and remote control

The phone apps work in two ways, with the same screens.

**On the phone alone** ("Work on this phone"): the agent runs in the app.
The loop, the file tools, the memory (`.cortex/memory.db` in each project),
the intuition, the decision controllers and the living network are the
same Rust code the desktop runs, built for the phone; only the model is
elsewhere. Under **This phone** choose ChatGPT (signed in to in a page the
app opens, as `cortex login` does), Anthropic or a hosted service with a
key, and make projects, which live in the app's own folder. While the
phone is idle the system wakes the app now and then (WorkManager on
Android, a background refresh task on iOS) and the living network goes
over what it met. What a phone cannot do is left out of its list of
capabilities: the project's own checks (`gate`), `github`, `browser` and
`rollouts` need programs a phone does not have, and `mind`, `semantic` and
`hope` need the Python services, so memory compares tasks by shared words
there. There are no compilers, package managers or git on a phone, and iOS
lets an app start no other program at all, so the agent writes and edits
code there but cannot build or test it; connect to a computer for that. A
run stops if the system suspends the app, and API keys are kept in a file
in the app's private folder, not yet in the system's keychain.

**Connected to a computer**: the desktop app can be followed and driven
from a phone. The agent, its tools, its services and your API keys stay on
the computer; the phone is a second window onto the chats that are open
there. It can send instructions,
steer or stop a run, answer the agent's questions, switch models and every
capability, open past sessions and look at the memory.

Switch it on in the desktop app under Settings → Remote control, show a
pairing code and scan it with the phone app. The connection (`iroh`) is
dialed by public key and encrypted end to end. It goes directly when both
are on one network and through a public relay when they are not, so no
port is opened. A code lets one phone in, once, for ten minutes; after that
the phone is known by its key and can be forgotten from the same page.

    crates/cortex-remote    the messages, pairing, and the link; also the
                            conversation model both sides build from events
    crates/cortex-mobile    the phone apps' core, exposed to Swift and Kotlin:
                            the agent on the phone (`src/local`) and the
                            connection to a desktop
    apps/ios                the SwiftUI app
    apps/android            the Jetpack Compose app

iOS, with full Xcode and `brew install xcodegen`:

    rustup target add aarch64-apple-ios aarch64-apple-ios-sim
    scripts/mobile-ios.sh
    cd apps/ios && xcodegen && open Cortex.xcodeproj

Android, with a JDK, the Android SDK and NDK, and `cargo install cargo-ndk`:

    rustup target add aarch64-linux-android
    scripts/mobile-android.sh
    cd apps/android && ./gradlew :app:assembleDebug

Pictures of both are in `apps/screenshots`.

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
| `--with brain` | The dendritic network, allowed to speak: when its own record says it is sure enough, one line between steps says what has usually come next here and gone well, or that the usual next step has usually gone badly. See *Dendritic network* above. It replaced a spiking network the size of the fruit fly brain (139,255 neurons, 300 MB), which predicted no better than the 17-number controller on saved runs and was removed | 
| `--with intuition` | A second brain beside the controller: an associative network with a neuron for each task word, file, command and error it has met, and synapses strengthened by passing runs, strengthened at half rate by completed unchecked runs, and weakened by failures. Before a task it names the files that tasks worded like it went on to change; between steps, what passing runs went on to from the file just read or the error just met; on a long run, a check left failing or edits nothing has checked. Silent until it has learned from four runs. Works with or without `--with brain` |
| `--with hope` | Let the HOPE-style memory model re-score recalled memories. Needs `--with memory` and the service in [`ml/`](ml/README.md). Untrained, it ranks exactly like plain similarity |
| `--with mind` | The learning brain from the [`brain`](../brain/README.md) project. **On in `run`, `chat` and the app** unless turned off with `--without mind` or the switch in Settings → Capabilities; off in `eval` unless asked for. It watches every step and hears what you say: it learns which actions work here, keeps your corrections, and treats your next message after an unchecked run as the verdict on it. It shows what earlier work found before a task, and between steps says a line when it has one: a habit it is sure of, something you said before, an action that failed here before, a choice worth asking you about, something untried. The model can have it imagine how actions would turn out through `brain_state`. Its service is started when not running (`CORTEX_BRAIN_DIR` if the brain project is not beside this one, `--mind-url` for another port). It stays silent in a share of runs (`--mind-control`, default 0.2) so `cortex mind` can compare checks passed, steps, tokens and time with and without it. Without `--with gate` it learns from commands and from you, not from checks |
| `--with skills` | Let the model pick a skill itself: the list of skills goes in the system prompt (about 350 tokens) and the `skill` tool loads one. Typing `/name` works without this |
| `--with github` | `github` reads pull requests, their diffs, checks and discussion, issues and workflow logs, without asking. `github_write` opens pull requests and issues and posts comments and reviews, and asks every time unless `--yes` is given. Both run the `gh` command. Also adds rules for git to the system prompt: commit and push only when asked, never force-push |
| `--with browser` | A real web browser, for looking at an interface, testing it and automating work on web pages. `browser` opens an address or a file in the project, clicks, types, presses keys, chooses from lists, resizes the page and runs scripts, and asks every time unless `--yes` is given. `browser_view` reads the page as text (one line per control, heading and run of text, with a number on each control to act on it by), takes screenshots into `.cortex/browser/`, reports console errors and failed requests, and waits for something to appear, without asking. Uses the installed Chrome, Chromium, Edge or Brave (`CORTEX_BROWSER` names another) with an empty profile of its own, out of sight unless `--browser-window` is given. The model reads pages as text; `--browser-pictures` also shows it the screenshots, for a model that takes images |

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

## Package for another machine

One command builds both programs in release mode and packs them with
everything they need: a Python interpreter of their own, every library,
the embedding model's weights, the HOPE weights and the mind. The machine
that receives the package installs nothing and downloads nothing.

```sh
scripts/package.sh --zip       # macOS: dist/Cortex-<version>-macos-<arch>.zip
scripts/package.sh --zip       # Linux: dist/cortex-<version>-linux-<arch>.tar.gz
scripts\package.ps1 -Zip       # Windows: dist\Cortex-<version>-windows-x64.zip
```

Without `--zip` the package is left in `target/package`. `--install` on
macOS also copies the app to `/Applications`. A package runs on the system
and processor it was built on, so build on each kind of machine. The
building machine needs `cargo`, `uv` and the `brain` project beside this
checkout (or `CORTEX_BRAIN_DIR`).

On another Mac a downloaded copy is held back because it is not signed by
a registered developer. Once, in Terminal: `xattr -cr /Applications/Cortex.app`.

To only compile, without packaging:

```sh
cargo build --release -p cortex-gui -p cortex-cli
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
  cortex-brain/       the decision controller, the intuition network, and the client of the mind service 
  cortex-tools/       the tools the agent can call
  cortex-skills/      skills: the built-in ones (`skills/`), loading a project's own, `/name`, and the `skill` tool
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
| `ContextSource` | Put context in front of an instruction | memory, the router, a skill started with `/name` |
| `Verifier` | Check finished work | `cortex-verify` |
| `Adviser` | Add a note between steps | the decision controllers, the intuition and the mind in `cortex-brain` |

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
- The dendritic network's advice is gated by its own record, but that record is of whether the advised step came next, not of whether the task went better for it. No run against a model has measured its effect. `--with living` runs it silently for comparison. 
- The controller stays silent until it has learned from five checked runs, and learns nothing without `--with gate`. Replayed over 40 saved runs (`cargo run --release -p cortex-brain --example replay`), it predicted little better than a constant. 
- The intuition recalls what earlier runs found; it finds nothing new. Replayed over 24 saved tradelyzer runs (`--example intuition_replay`), it named a file before the task in 6 runs, all 6 correctly, and stayed silent on tasks it had never met. Those runs opened that file after 1.3 steps anyway, so on short tasks it saves about a step. Its effect on a model, and on long tasks, is unmeasured.
- The mind's effect is measured, not yet known. Every run is recorded under the arm it was in, speaking or silent, and `cortex mind` prints checks passed, steps, tokens and seconds for each; until both arms hold enough real runs the difference is noise. In an earlier replay over 38 saved tradelyzer runs (`--example mind_replay`), made before failing commands stopped counting against the action that ran them, its guess of the model's next action was right at 15% of 237 steps and it never spoke. What it adds for certain is a paragraph in the system prompt, its lines, and a few milliseconds per step, which `cortex mind` reports as `mind ms`.
- Responses are not streamed, so text appears a message at a time.
- The browser tools are tested against Chrome on macOS with small local pages, not with a model and not against a real application, so whether an agent tests an interface well with them is unmeasured. There is one page: a link meant for a new tab opens in it, and a window a script opens is not followed. Frames inside a page are named but not read. Files cannot be uploaded or downloaded. They do not work on Windows.
- The skills and the GitHub tools have not been run with a model. The `github` tool was tried against a public repository; `github_write` only against a stand-in for `gh`, since a real call posts in public. Whether a skill makes a review or a design better than the bare instruction is unmeasured.
- The smoke suite is small and easy. It proves the harness works; it is not a benchmark.
