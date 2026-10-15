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

**The brain** (`--with brain`, on in `run`, `chat` and the app unless turned off with `--without brain`; crate `cortex-brain`): one brain for everything that learns from experience. Chats, coding sessions and the trading desk all reach the same one, kept alive by one background service. Its cortex is a sheet of columns of spiking cells; each cell grows dendritic branches whose synapses learn, by permanence, which earlier activity predicts it, so the same word or percept in two contexts is two different sets of cells. From that it predicts what comes next, and an engram index keeps each lesson and each lived episode as the cells that held it, to be recalled from a cue. It speaks, recalls, thinks, imagines several continuations before committing to one, reasons along chains of links, follows hunches and reflects on its own record. It lives each agent run, and each market, as a stream of percepts (`senses`) with value and an eligibility trace (checks passing, a trade won, you agreeing are good news), holds up to four things in attention, and sets its own plasticity from how surprised it is. When idle it sleeps when drowsy: it replays memories it still predicts badly, prunes, and dreams. With it on, one line reaches the model between steps only when its own record of such hunches says it is usually right; the model can also ask it through `brain_state` (`imagine`, `goal`) and talk to it with `brain_chat`. One brain for all projects in `~/.cortex/brain/brain.bin`. Measured without a model on a few hundred short taught sentences: it predicts the next word of new text right 0.51 of the time on first hearing, against 0.25 for always guessing the commonest word, and 0.62 on held-out sentences after training. On a short instruction that follows a kind of run it sees coming after about 25 repetitions, and has not been wrong in tests. Whether its line helps a model is measured by `--holdout` and `cortex effect`, and not yet known. This is sequence learning over cells, not reasoning in language, and none of what it measures of itself is evidence of experience or awareness.

**Settings → Workspace → Share memory between projects** makes the agent also recall the closest tasks and general lessons from your other projects (`--memory-from <FOLDER>` on the command line, with `--with memory`), and draws all of them in the memory space. A project's facts about its own files are never shared.

When a run ends, the brain's guess at what you will ask for next shows in the empty composer; **Tab** puts it in the box to send or edit. It comes from what you asked after earlier runs that went the same way, and only once it is sure. Needs the `brain` capability.

Set it up on its **Settings** page: the provider, its API key (kept in the system keychain, not in a file), the models to offer, permissions, and the same optional capabilities `--with` switches on. The other choices are saved in `~/.cortex/app.json`. Arguments that work for `cortex run` (`--provider`, `--model`, `--base-url`, `--with`, `--dir`, `--yes`) are accepted too and are taken over into the settings.

The sidebar lists project folders, each with its own sessions, and below them the sessions that belong to no project (these work in `~/.cortex/sessions`). Under the message box are the speed of the latest model call, the share of input served from the provider's cache, the loop the agent is running, and selectors for the model, the reasoning effort (Anthropic only) and whether to ask before changes.

Use the **...** menu beside the view controls (or **⌘N**) to start another independent agent. The menu switches between open chats, closes the current chat, and selects **Single pane**, **Side by side**, or **Top and bottom**. Chats keep running when you switch. Split panes have separate composers, approvals, model choices and stop buttons; drag the divider to resize them. Click inside a pane to make its project and session active in the sidebar. Saved sessions remain in the sidebar. `cargo run -p cortex-gui -- --split` starts with two panes.

Agents in the same project can collaborate through `team_list_agents`, `team_send_message` and `team_read_messages`. For example, ask one chat to implement an API and another to review its tests, and tell them to coordinate file ownership with their peers. Messages reach a working agent between steps and start a turn for an idle agent. Each agent keeps its own conversation and trajectory; peers share project files and memory, so they should agree on which files each will edit. Agents in different projects cannot send messages to each other.

A saved session can be continued by clicking it in the sidebar. If its chat is already open, that chat is selected; otherwise a new independent worker resumes its conversation. Search filters projects and saved sessions. During a run, new messages are queued until the model reads them, and stopping returns unread messages to the composer. Provider reasoning streams live and can be hidden; it is not stored in the log. Settings are grouped into Models & accounts, Permissions and Capabilities. Settings applied during a run take effect when that agent finishes.

Memory is always on in the app, in the same `.cortex/memory.db` the command line uses. New desktop sessions also enable Brain and Gate by default; Capabilities can switch them off. Settings saved by earlier versions migrate once: `mind` becomes `brain`, and the retired `intuition`, `living` and `controller` are dropped.

**More → Brain**, or **⌘⇧L**, draws the brain live: its cortex as a sheet of columns lighting up as words and percepts pass, the cells primed by a dendritic spike (what it expects next), a chosen memory's cells, and beside it its mind: time as it felt, attention, values, its advice record, its senses and streams, its thoughts, and buttons to make it think, dream, reflect, imagine, follow a hunch or reason from one thing to another. **Metrics** shows every number behind it. Positions are a learned map, not anatomy.

The brain belongs to a local background service, shared by the desktop app, the command line and the trading desk. Ending a run or closing the window does not end it. Events enter a durable private inbox, and each is moved to the archive only after the brain that lived it has been saved, so none is lost if the service stops. One writer updates the brain. While nothing uses it, it thinks and sleeps on its own. Replay does not count as fresh evidence or improve the reported accuracy by itself.

It learns language from what it hears: your messages and the agent's replies in every chat, the trading conversation, and an optional LLM teacher (`/teach snn+dann [topic]` in a chat, `/teach stop` to end it) that gives it stories, quizzes it and corrects it. `/brain <words>` talks to it, `/recall <cue>` asks it to remember, `/think`, `/dream`, `/reflect`, `/imagine`, `/intuit` and `/reason a -> b` ask its mind. Teacher judgments are not verified. It is a small learner, not a fluent LLM.

Grounded conversation now combines the sequence learner with explicit skills. It executes bounded exact arithmetic, explains counting operations, tracks simple object collections across turns, indexes taught dialogue and declarative premises, and executes learned formula rules with SI dimension checks. These algorithms are distinct from learning the rules in neural weights. Try `/brain why is 1 + 3 = 4?`, `/learn force = mass * acceleration`, then `/brain what is force if mass is 2 kg and acceleration is 3 m/s^2?`. `/brain` enters continuous conversation, and `/agent` returns to the agent. In the Brain view, **Study introductory lessons** supplies 25 small conversation and mechanics lessons, and the conversation and evidence panels show which mechanism answered, the steps, source memories, unresolved questions, and rehearsal measurements. Unknown questions and conflicting premises remain explicit. General English comprehension, arbitrary word-problem solving, and subjective awareness have not been established.

`cortex-brain-service foundations` studies the same introductory curriculum. `study '<lesson>'` uses up to four rehearsal passes and reports prediction before and after on that lesson; this is retention, not held-out accuracy. `--file /absolute/path/brain.bin` selects an isolated brain for experiments. Checkpoint version 2 atomically saves the cortex, mind, conversation state and budget together; legacy cortex-plus-JSON snapshots still load. Older executables cannot read version 2, so retain a backup before upgrading the service.

`scripts/prepare-brain-research.py <datasets directory>` prepares bounded local GSM8K, Dolly and SciBench samples with separate training and evaluation data. `cargo run -p cortex-brain --example learning_bench -- target/brain-research` writes a report and experimental checkpoint. It reports supplied-equation execution separately from answering the original word problems, and compares one exposure, adaptive study, fixed rehearsal, and additional replay on frozen sequence probes. See `BRAIN_IMPLEMENTATION_STATUS.md` for measured scope and limitations.

Build and manage the service independently:

```bash
cargo build --release -p cortex-brain
target/release/cortex-brain-service install   # macOS: start now and at login
target/release/cortex-brain-service status
target/release/cortex-brain-service brain     # what it has learned so far
target/release/cortex-brain-service import .cortex/trajectories
target/release/cortex-brain-service chat      # talk with it in the terminal
target/release/cortex-brain-service say 'where is the cat'
target/release/cortex-brain-service recall 'the cat'
target/release/cortex-brain-service stop
```

The macOS service is installed in `~/.cortex/bin/` with LaunchAgent `dev.cortex.brain`, which replaces the retired `dev.cortex.living-network`; the brain, its inbox, archive and log are under `~/.cortex/brain/`. On first start a brain left in `~/.cortex/living/` is taken over. `start` runs it without installing login startup. In `chat`, `/right` or `/wrong <better answer>` teaches it from its last reply. In Claude Code, `/talk-to-brain <words>` relays a conversation through the project skill in `.claude/skills/`. Imports of the same unchanged saved trajectory are idempotent; imported history is only heard, not lived again. Background work pauses while the computer sleeps and resumes when it wakes.

Opening an old chat restores its recorded working memory. Persistent memory includes chat-only answers and the last five successful calculation results per task. `calculate` evaluates numeric arithmetic locally using bounded double-precision parsing; it does not execute code. `brain_state` exposes bounded diagnostics when the brain is on. What the brain made of each step is logged as `brain_observed` events, never inserted automatically into the model prompt.

SQLite consolidates episodes and derived facts in one transaction. The brain is one per machine and shared between chats. These observations are not a controlled accuracy comparison; use the evaluation harness to compare enabled capabilities against the baseline.

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
| `--with brain` | The one brain (see *The brain* above). **On in `run`, `chat` and the app** unless turned off with `--without brain`. It lives every step and hears what you say; when its own record says it is sure enough, one line between steps says what has usually come next here and gone well, or that the usual next step has usually gone badly. `brain_state` lets the model ask it to imagine where actions lead or how an outcome is reached, and `brain_chat` to talk to it. Its service is started when not running; without it, the brain lives in the process. It replaced the living network, the decision controller, the intuition network and the `mind` service |
| `--with hope` | Let the HOPE-style memory model re-score recalled memories. Needs `--with memory` and the service in [`ml/`](ml/README.md). Untrained, it ranks exactly like plain similarity |
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
  cortex-brain/       the one brain (`brain/`), how the agent and markets reach it (`senses`), its explicit memory of past tasks (`memory/`: working, episodic and long-term; consolidation; recall), the link and protocol everything uses, and its background service
  cortex-verify/      the verification gate: finds and runs a project's checks
  cortex-router/      picks and gathers context for a task before the model starts
  cortex-liquid/      market models (CfC, spiking, ridge), read alongside the brain
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
| `Adviser` | Add a note between steps | the brain (`Counsel` in `cortex-brain`) |

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
- The brain's advice is gated by its own record, but that record is of whether the advised step came next, not of whether the task went better for it. No run against a model has measured its effect yet; `--holdout` and `cortex effect` are how to measure it. It suggests a request it has seen follow a kind of run after about five repetitions, and stays silent where it has seen it go more than one way. Measured without a model, a second set of lessons reusing the first set's words costs the first about 0.01 of its next-word accuracy, and sleep improves both sets; its top guess for the next word of never-seen text is right 0.38 of the time, against 0.25 for the commonest word, because it keeps every word that can follow in play.
- Responses are not streamed, so text appears a message at a time.
- The browser tools are tested against Chrome on macOS with small local pages, not with a model and not against a real application, so whether an agent tests an interface well with them is unmeasured. There is one page: a link meant for a new tab opens in it, and a window a script opens is not followed. Frames inside a page are named but not read. Files cannot be uploaded or downloaded. They do not work on Windows.
- The skills and the GitHub tools have not been run with a model. The `github` tool was tried against a public repository; `github_write` only against a stand-in for `gh`, since a real call posts in public. Whether a skill makes a review or a design better than the bare instruction is unmeasured.
- The smoke suite is small and easy. It proves the harness works; it is not a benchmark.

### Circuit-only research lab

The separate `cortex-brain-circuit` binary trains output synapses over a sparse
membrane/dendrite simulation without a pretrained language model. It supports
limited question-to-operation learning, supplied exact arithmetic, corrections,
replay and inspection of every simulated connection. It has not achieved broad
unseen English, maths or physics understanding. See
[BRAIN_IMPLEMENTATION_STATUS.md](BRAIN_IMPLEMENTATION_STATUS.md) for measured
successes and failures, and [the circuit plan](BRAIN_CIRCUIT_PLAN.md) for remaining
research gates.

```sh
python3 scripts/prepare-circuit-research.py /Users/harshevo/ai_projects/research_training_data/datasets
cargo run --release -p cortex-brain --bin cortex-brain-circuit -- train target/circuit-research target/circuit-research/circuit.json 100
cargo run --release -p cortex-brain --bin cortex-brain-circuit -- evaluate target/circuit-research target/circuit-research/circuit.json
cargo run --release -p cortex-brain --bin cortex-brain-circuit -- serve target/circuit-research/circuit.json
```

Open the Cortex desktop Brain view, or use the binary's `chat CHECKPOINT` command.
The desktop uses local Unix socket IPC at `~/.cortex/brain/circuit.sock`.
Override `CORTEX_CIRCUIT_SOCKET` in both processes to use a different socket.
`train` creates a fresh experimental checkpoint; chat corrections and replay
persist to the selected checkpoint. The normal brain service uses its own file.

The next experiment adds **one shared recurrent core** with operation and English
token readouts, surrogate-gradient training of recurrent/dendritic synapses,
branch gains and neuronal thresholds, and CPU/MPS training benchmarks. It uses
the local datasets without a pretrained model. English generation remains
incoherent and broad reasoning has not improved enough to replace the first lab.
See the [training protocol](ml/cortex_ml/circuit/README.md) and
[measured comparison](docs/research/brain-plastic-circuit-2026-10-15.json).
The live checkpoint is served through local IPC; the circuit no longer exposes HTTP.
