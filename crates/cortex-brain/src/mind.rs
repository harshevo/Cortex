//! The learning brain kept in the `brain` project, reached over local HTTP.
//!
//! The controllers and the intuition in this crate each learn one thing
//! from finished runs. The mind is a separate service that watches every
//! step as it happens. Before the model answers, it guesses the next
//! action; before a command's result or the checks come back, it predicts
//! them; it stores each step more strongly the more it was surprised; and
//! when the run ends it shares the outcome back along the steps. See the
//! `brain` project's README for what it does with that between runs.
//!
//! Here it is three things at once:
//!
//! - a [`TrajectorySink`], which is how it sees the run, and how it hears
//!   the person: what they say during a run is learned as a lesson about
//!   the step just taken, and what they say next, after a run nothing
//!   checked, settles whether that run was any good;
//! - an [`Adviser`], which says a line when the brain has one worth
//!   saying and nothing otherwise: what the person said the last time, a
//!   habit it is sure of, an action it predicted would fail and did, a
//!   choice worth asking the person about, or something untried worth
//!   trying. Through the `brain_state` tool the model can also have it
//!   imagine how actions it is weighing would turn out;
//! - a guess at what the person will ask for next ([`Mind::suggest`]),
//!   from what they asked after earlier runs that ended the same way.
//!   That one is for the person, so the control arm does not silence it;
//! - a [`ContextSource`], which puts what earlier work found in front of
//!   the task.
//!
//! So that its effect can be measured, it stays silent for a share of
//! runs, chosen at random: the control arm. It watches and learns in
//! those all the same, and every run's result, steps, tokens and time are
//! kept by the service under the arm the run was in.
//!
//! It never acts and must never fail a run: when the service stops
//! answering, the mind goes quiet until the next run.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use cortex_core::{
    Advice, Adviser, Briefing, ContextSource, Event, ToolCall, ToolResult, TrajectorySink,
    Workspace,
};
use serde_json::{Value, json};

pub const DEFAULT_URL: &str = "http://127.0.0.1:8767";

/// The share of runs in which the mind says nothing, to compare with.
pub const DEFAULT_CONTROL: f64 = 0.2;

/// Told to the model once, in the system prompt, when the mind is on.
pub const PROMPT_NOTE: &str = "A learning brain, the mind, watches this work and remembers how \
earlier work here went and what the person said about it. Lines that begin `Mind:` come from it: \
weigh them as experience, not as orders. When you are unsure which of a few next actions to take, \
you can call `brain_state` with `imagine` set to those actions, written as `tool: target` (for \
example `shell: cargo test` or `edit_file: src/lib.rs`), to see how each is predicted to turn out \
here.";

const CONNECT_TIMEOUT: Duration = Duration::from_millis(300);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

/// How long a service that was just started is given to answer. Loading
/// its libraries takes a few seconds the first time.
const START_TIMEOUT: Duration = Duration::from_secs(20);

/// How much of the instruction, of an action and of a result goes into the
/// text the brain compares situations by. The instruction is kept short so
/// that what just happened weighs more than which task it happened in:
/// "after a failing test" should resemble itself across tasks.
const TASK_CHARS: usize = 60;
const ACTION_CHARS: usize = 160;
const RESULT_CHARS: usize = 120;

/// Most lines of each kind in one run. A question or a nudge to explore
/// is worth hearing once; repeated, it is noise the model pays for.
const ASKS_PER_RUN: u32 = 1;
const NUDGES_PER_RUN: u32 = 2;

pub struct Mind {
    /// `host:port` of the service.
    address: String,

    /// Which repository this is, so that what one project taught is not
    /// taken for knowledge about another.
    scope: String,

    /// The share of runs that are told nothing.
    control: f64,

    /// A request got no answer: say nothing more this run.
    down: AtomicBool,

    /// Time spent waiting for the service this run, in microseconds.
    spent: AtomicU64,

    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    instruction: String,

    /// The brain's number for this run.
    task: Option<u64>,

    /// The run before this one, which the person's next words may judge.
    previous: Option<u64>,

    /// This run is in the control arm: watched, and told nothing.
    silent: bool,

    /// What the latest tool call did, as the brain is told it.
    last: Option<String>,

    /// Tool call id to what the brain made of the call.
    steps: HashMap<String, Step>,

    /// The step in which the model said it was done, waiting for the checks.
    finish: Option<u64>,

    /// The brain predicted that an action would fail, and it did not
    /// succeed: said at the next opportunity.
    warning: Option<String>,

    /// Lines already said this run, so none is said twice.
    said: HashSet<String>,

    asks: u32,
    nudges: u32,

    /// Lines shown to the model this run, the briefing included.
    advised: u32,

    /// A tool changed a file in this run.
    edited: bool,

    /// How the run ended, once it has: what the person's next request is
    /// learned against.
    ending: Option<Ending>,

    /// What was suggested as their next request, if anything was.
    suggested: Option<String>,
}

/// How a run ended, as the brain compares endings.
#[derive(Clone)]
struct Ending {
    text: String,

    /// Whether the checks passed. `None` when nothing checked the work.
    checked: Option<bool>,

    edited: bool,
}

struct Step {
    id: Option<u64>,
    action: String,
    warning: Option<String>,
}

/// How a tool call turned out, as far as its result says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Worked,

    /// A command exited non-zero. That may be exactly what it was run to
    /// find out, so the run's outcome decides what it was worth.
    Exited,

    /// The tool itself failed, or the person declined it.
    Failed,
}

impl State {
    /// Where the run stands, in words: the task and what just happened.
    fn situation(&self) -> String {
        match &self.last {
            Some(last) => format!("after {last} | task: {}", self.instruction),
            None => format!("at the start | task: {}", self.instruction),
        }
    }
}

impl Mind {
    pub fn new(url: &str, scope: &str) -> Self {
        let address = url
            .trim_start_matches("http://")
            .trim_end_matches('/')
            .to_string();

        Self {
            address,
            scope: scope.to_string(),
            control: 0.0,
            down: AtomicBool::new(false),
            spent: AtomicU64::new(0),
            state: Mutex::new(State::default()),
        }
    }

    /// Stays silent for this share of runs, so the rest can be compared
    /// with them.
    pub fn with_control(mut self, share: f64) -> Self {
        self.control = share.clamp(0.0, 1.0);
        self
    }

    /// What the brain holds, or `None` when the service is not running.
    pub fn health(&self) -> Option<Value> {
        self.request("GET", "/health", None)
    }

    /// Starts the service on this machine if it is not answering, and
    /// waits for it. Returns what it holds, or why it could not be
    /// started. The service outlives this process, so only the first use
    /// waits.
    pub fn wake(&self) -> Result<Value, String> {
        if let Some(health) = self.health() {
            return Ok(health);
        }

        let (host, port) = self
            .address
            .rsplit_once(':')
            .ok_or_else(|| format!("{} has no port", self.address))?;

        if !matches!(host, "127.0.0.1" | "localhost") {
            return Err(format!("the service at {} is not answering", self.address));
        }

        let project = brain_project().ok_or(
            "the brain project was not found. Set CORTEX_BRAIN_DIR to the folder that holds it",
        )?;

        let python = std::env::var("CORTEX_PYTHON").unwrap_or_else(|_| "python3".to_string());

        let mut command = Command::new(&python);

        command
            .args(["-m", "cortex.brain.server", "--port", port])
            .current_dir(&project)
            .stdin(Stdio::null());

        match service_log() {
            Some((out, err)) => command.stdout(out).stderr(err),
            None => command.stdout(Stdio::null()).stderr(Stdio::null()),
        };

        // Its own process group, so that Ctrl-C in the terminal stops the
        // agent and leaves the brain running for the next one.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);

        command
            .spawn()
            .map_err(|error| format!("cannot run `{python}` in {}: {error}", project.display()))?;

        let started = Instant::now();

        while started.elapsed() < START_TIMEOUT {
            std::thread::sleep(Duration::from_millis(250));

            if let Some(health) = self.health() {
                return Ok(health);
            }
        }

        Err(format!(
            "the service was started from {} and did not answer within {} seconds; see \
             ~/.cortex/mind/server.log",
            project.display(),
            START_TIMEOUT.as_secs()
        ))
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn is_down(&self) -> bool {
        self.down.load(Ordering::Relaxed)
    }

    fn post(&self, path: &str, body: Value) -> Option<Value> {
        self.request("POST", path, Some(&body))
    }

    /// One request, one connection. The service is on this machine, so a
    /// plain blocking exchange is quicker than it is worth avoiding. A
    /// request that gets no answer at all marks the service as down, so
    /// that a run never waits on it twice.
    fn request(&self, method: &str, path: &str, body: Option<&Value>) -> Option<Value> {
        let started = Instant::now();
        let answer = self.exchange(method, path, body);

        self.spent
            .fetch_add(started.elapsed().as_micros() as u64, Ordering::Relaxed);

        match answer {
            Some((true, value)) => Some(value),
            Some((false, _)) => None,
            None => {
                self.down.store(true, Ordering::Relaxed);

                None
            }
        }
    }

    /// The service's answer and whether it was a success, or `None` when
    /// there was no answer.
    fn exchange(&self, method: &str, path: &str, body: Option<&Value>) -> Option<(bool, Value)> {
        let address = self.address.to_socket_addrs().ok()?.next()?;

        let mut stream = TcpStream::connect_timeout(&address, CONNECT_TIMEOUT).ok()?;

        stream.set_read_timeout(Some(REQUEST_TIMEOUT)).ok()?;
        stream.set_write_timeout(Some(REQUEST_TIMEOUT)).ok()?;

        let payload = body.map(Value::to_string).unwrap_or_default();

        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n",
            self.address,
            payload.len()
        );

        stream.write_all(head.as_bytes()).ok()?;
        stream.write_all(payload.as_bytes()).ok()?;

        let mut response = Vec::new();

        stream.read_to_end(&mut response).ok()?;

        let text = String::from_utf8_lossy(&response);
        let (head, body) = text.split_once("\r\n\r\n")?;
        let ok = head.lines().next()?.contains(" 200 ");

        Some((ok, serde_json::from_str(body).unwrap_or(Value::Null)))
    }

    /// Tells the brain which action was taken. The brain predicts the
    /// outcome on receiving it, before any result is reported, and says
    /// so when it expects the action to fail.
    fn step(&self, task: Option<u64>, situation: &str, action: &str, file: &str) -> Step {
        let reply = self.post(
            "/step",
            json!({ "task": task, "situation": situation, "action": action, "file": file }),
        );

        let field = |name: &str| reply.as_ref().map(|reply| &reply[name]);

        Step {
            id: field("step").and_then(Value::as_u64),
            action: action.to_string(),
            warning: field("warning").and_then(Value::as_str).map(str::to_string),
        }
    }

    fn result(&self, step: u64, verdict: Verdict) {
        let outcome = if verdict == Verdict::Worked { 1.0 } else { 0.0 };

        self.post(
            "/result",
            json!({ "step": step, "outcome": outcome, "soft": verdict == Verdict::Exited }),
        );
    }

    /// Whether a run that starts now is one the mind stays silent in.
    fn draw_control(&self) -> bool {
        let moment = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|since| since.subsec_nanos())
            .unwrap_or_default();

        f64::from(moment % 1000) < self.control * 1000.0
    }
}

impl TrajectorySink for Mind {
    fn record(&self, event: &Event) {
        let mut state = self.state();

        match event {
            Event::RunStarted { instruction, .. } => {
                let previous = state.task.take().or(state.previous);

                self.down.store(false, Ordering::Relaxed);
                self.spent.store(0, Ordering::Relaxed);

                // What they ask for now is what followed the way the last
                // run ended: the next suggestion is drawn from it.
                if let Some(ending) = state.ending.take() {
                    self.post(
                        "/asked",
                        json!({
                            "ending": ending.text,
                            "message": instruction,
                            "repo": self.scope,
                            "shown": state.suggested.take(),
                        }),
                    );
                }

                // What the person says next is the only verdict a run that
                // nothing checked ever gets.
                if let Some(previous) = previous {
                    self.post(
                        "/feedback",
                        json!({ "task": previous, "text": instruction, "after": true }),
                    );
                }

                let silent = self.draw_control();

                let begun = self.post(
                    "/begin",
                    json!({
                        "goal": instruction,
                        "repo": self.scope,
                        "arm": if silent { "control" } else { "advised" },
                        "several": true,
                    }),
                );

                *state = State {
                    instruction: clip(instruction, TASK_CHARS),
                    task: begun.as_ref().and_then(|begun| begun["task"].as_u64()),
                    previous,
                    silent,
                    ..State::default()
                };

                if state.task.is_none() {
                    self.down.store(true, Ordering::Relaxed);
                }
            }

            Event::ModelResponded { message, .. } if !self.is_down() => {
                let situation = state.situation();
                let calls = message.tool_calls();

                // No tool call means the model says it is done. Whether it
                // is right is what the checks will tell.
                if calls.is_empty() {
                    state.finish = self.step(state.task, &situation, "finish", "").id;
                }

                for call in calls {
                    let (action, file) = describe(&call);
                    let step = self.step(state.task, &situation, &action, &file);

                    state.steps.insert(call.id, step);
                }
            }

            Event::ToolFinished { tool, result, .. } if !self.is_down() => {
                let Some(step) = state.steps.remove(&result.call_id) else {
                    return;
                };

                let verdict = verdict(tool, result);

                if let (Some(id), Some(verdict)) = (step.id, verdict) {
                    self.result(id, verdict);
                }

                if matches!(tool.as_str(), "edit_file" | "write_file") && !result.is_error {
                    state.edited = true;
                }

                // A prediction of failure is worth saying once it has not
                // been proved wrong.
                if verdict != Some(Verdict::Worked) && state.warning.is_none() {
                    state.warning = step.warning;
                }

                state.last = Some(summary(&step.action, verdict, result));
            }

            Event::VerificationRan { passed, checks, .. } if !self.is_down() => {
                if let Some(step) = state.finish.take() {
                    self.result(
                        step,
                        if *passed {
                            Verdict::Worked
                        } else {
                            Verdict::Failed
                        },
                    );
                }

                let failure = checks.iter().find(|check| !check.passed);

                state.last = Some(match failure {
                    Some(check) => format!(
                        "the checks: `{}` failed: {}",
                        check.command,
                        clip(first_line(&check.output), RESULT_CHARS)
                    ),
                    None => "the checks passed".to_string(),
                });
            }

            Event::UserInterjected { text, .. } => {
                if !self.is_down() {
                    self.post("/feedback", json!({ "task": state.task, "text": text }));
                }

                state.last = Some(format!("the person said: {}", clip(text, RESULT_CHARS)));
            }

            Event::RunFinished {
                verified,
                steps,
                usage,
                duration_ms,
                ..
            } if !self.is_down() => {
                let outcome = verified.map(|passed| if passed { 1.0 } else { 0.0 });

                self.post(
                    "/end",
                    json!({
                        "task": state.task,
                        "outcome": outcome,
                        "n_steps": steps,
                        "tokens_in": usage.total_input(),
                        "tokens_out": usage.output_tokens,
                        "duration_ms": duration_ms,
                        "advised": state.advised,
                        "mind_ms": self.spent.load(Ordering::Relaxed) / 1000,
                    }),
                );

                state.previous = state.task.take();

                let how = match (verified, state.edited) {
                    (Some(true), _) => "the checks passed",
                    (Some(false), _) => "the checks failed",
                    (None, true) => "files were edited and nothing checked them",
                    (None, false) => "nothing was edited and nothing was checked",
                };

                state.ending = Some(Ending {
                    text: format!("{how} | task: {}", state.instruction),
                    checked: *verified,
                    edited: state.edited,
                });
            }

            _ => {}
        }
    }
}

impl Mind {
    /// The brain's whole thought about where the run stands now: its guess
    /// of the next action, how sure it is, and what it would say. `None`
    /// when the service is not answering.
    pub fn consider(&self) -> Option<Value> {
        if self.is_down() {
            return None;
        }

        let (task, situation) = {
            let state = self.state();

            (state.task, state.situation())
        };

        self.post("/advise", json!({ "task": task, "situation": situation }))
    }

    /// How the brain predicts each of `actions` would turn out from where
    /// the run stands, best first. Nothing is recorded.
    pub fn imagine(&self, actions: &[String]) -> Option<Value> {
        if self.is_down() || actions.is_empty() {
            return None;
        }

        let (task, situation) = {
            let state = self.state();

            (state.task, state.situation())
        };

        self.post(
            "/imagine",
            json!({ "task": task, "situation": situation, "actions": actions, "repo": self.scope }),
        )
    }

    /// What the person is likely to ask for now that the run has ended,
    /// or `None` when the brain has no clear guess. What it answers is
    /// remembered, so that their actual request can be held against it.
    pub fn suggest(&self) -> Option<String> {
        let ending = self.state().ending.clone()?;

        // Asked between runs: a service that does not answer here says
        // nothing about the next run, so it is not marked as down.
        let (ok, reply) = self.exchange(
            "POST",
            "/suggest",
            Some(&json!({
                "ending": ending.text,
                "repo": self.scope,
                "checked": ending.checked,
                "edited": ending.edited,
            })),
        )?;

        let suggestion = reply["message"].as_str().filter(|_| ok).map(str::to_string);

        self.state().suggested = suggestion.clone();

        suggestion
    }

    /// The conversation has moved to another session: what is asked next
    /// does not follow from how the last run here ended.
    pub fn turn_away(&self) {
        let mut state = self.state();

        state.ending = None;
        state.suggested = None;
    }

    /// How runs have gone with the mind speaking and with it silent.
    pub fn measured(&self) -> Option<Value> {
        Some(self.health()?["measured"].take())
    }
}

impl Adviser for Mind {
    /// Asked after a step's tools have run and before the model decides,
    /// so the guess is on record before the model's own choice is seen.
    fn advise(&self, _step: u32, _max_steps: u32) -> Option<Advice> {
        if self.state().silent {
            return None;
        }

        let thought = self.consider()?;
        let mut state = self.state();
        let mut lines: Vec<String> = Vec::new();

        if let Some(warning) = state.warning.take() {
            lines.push(warning);
        }

        if let Some(line) = thought["advice"].as_str() {
            let allowed = match thought["kind"].as_str() {
                Some("ask") => bump(&mut state.asks, ASKS_PER_RUN),
                Some("curious") => bump(&mut state.nudges, NUDGES_PER_RUN),
                _ => true,
            };

            if allowed {
                lines.push(line.to_string());
            }
        }

        lines.retain(|line| state.said.insert(line.clone()));

        if lines.is_empty() {
            return None;
        }

        state.advised += lines.len() as u32;

        Some(Advice {
            source: "mind".to_string(),
            text: lines.join("\n"),
            detail: thought,
        })
    }

    /// Answers the model's own question through the `brain_state` tool:
    /// given `imagine`, how those actions are predicted to turn out here.
    fn inspect(&self, query: &Value) -> Option<Value> {
        let actions: Vec<String> = query["imagine"]
            .as_array()?
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();

        if self.state().silent {
            return None;
        }

        let mut imagined = self.imagine(&actions)?;

        imagined["source"] = "mind".into();

        Some(imagined)
    }
}

/// Counts one more against `limit`, and says whether there was room.
fn bump(count: &mut u32, limit: u32) -> bool {
    if *count >= limit {
        return false;
    }

    *count += 1;

    true
}

#[async_trait]
impl ContextSource for Mind {
    async fn brief(&self, instruction: &str, _workspace: &Workspace) -> Option<Briefing> {
        if self.state().silent {
            return None;
        }

        // Asked about the situation the run is about to be in, so that what
        // comes back is what earlier runs did from there.
        let start = State {
            instruction: clip(instruction, TASK_CHARS),
            ..State::default()
        };

        let reply = self.post(
            "/brief",
            json!({ "goal": start.situation(), "repo": self.scope }),
        )?;

        let text = reply["text"].as_str()?.to_string();

        self.state().advised += 1;

        Some(Briefing {
            source: "mind".to_string(),
            text,
            detail: json!({ "scope": self.scope }),
        })
    }
}

/// The brain project: where `CORTEX_BRAIN_DIR` says, else `~/.cortex/brain`,
/// else beside the checkout this was built from.
pub fn brain_project() -> Option<PathBuf> {
    let named = std::env::var_os("CORTEX_BRAIN_DIR").map(PathBuf::from);
    let home = std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cortex/brain"));
    let beside = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../brain"));

    named
        .into_iter()
        .chain(home)
        .chain([beside])
        .find(|folder| folder.join("cortex/brain/server.py").is_file())
}

/// Where a service started from here writes what it prints.
fn service_log() -> Option<(std::fs::File, std::fs::File)> {
    let folder = PathBuf::from(std::env::var_os("HOME")?).join(".cortex/mind");

    std::fs::create_dir_all(&folder).ok()?;

    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(folder.join("server.log"))
        .ok()?;

    Some((log.try_clone().ok()?, log))
}

/// A tool call as the brain compares actions: the tool and what it was
/// pointed at, without the bulk of its input. Also the file, when there is
/// one.
pub fn describe(call: &ToolCall) -> (String, String) {
    let field = |name: &str| call.input.get(name).and_then(Value::as_str);

    let file = field("path").unwrap_or_default().to_string();

    let target = field("command")
        .or(field("path"))
        .or(field("pattern"))
        .or(field("query"))
        .or(field("name"))
        .unwrap_or_default();

    let action = if target.is_empty() {
        call.name.clone()
    } else {
        format!("{}: {}", call.name, clip(target, ACTION_CHARS))
    };

    (action, file)
}

/// Whether the call did what it was for, when its result says. A command
/// reports an exit code; any tool can fail outright. A read that returned
/// text says nothing about whether reading it was a good idea, so it is
/// left for the run's outcome to judge.
fn verdict(tool: &str, result: &ToolResult) -> Option<Verdict> {
    if result.is_error {
        return Some(Verdict::Failed);
    }

    if tool != "shell" {
        return None;
    }

    // The shell tool puts the exit code on the first line.
    let code = result
        .output
        .lines()
        .next()?
        .trim()
        .strip_prefix("exit code: ")?;

    Some(if code.trim() == "0" {
        Verdict::Worked
    } else {
        Verdict::Exited
    })
}

fn summary(action: &str, verdict: Option<Verdict>, result: &ToolResult) -> String {
    match verdict {
        Some(Verdict::Failed) if result.output.contains(DECLINED) => {
            format!("`{action}` was declined by the person")
        }

        Some(Verdict::Failed | Verdict::Exited) => format!(
            "`{action}` failed: {}",
            clip(telling_line(&result.output), RESULT_CHARS)
        ),

        _ => format!("`{action}`"),
    }
}

/// How a tool result reads when the person said no to the call.
const DECLINED: &str = "the user declined this action";

fn first_line(text: &str) -> &str {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
}

/// The line of a failure's output that says most: the first that mentions
/// an error, otherwise the first that is not the exit code.
fn telling_line(output: &str) -> &str {
    let lines = || {
        output
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with("exit code:"))
            .filter(|line| !matches!(*line, "stdout:" | "stderr:"))
    };

    lines()
        .find(|line| {
            let lower = line.to_lowercase();

            lower.contains("error") || lower.contains("failed") || lower.contains("panic")
        })
        .or_else(|| lines().next())
        .unwrap_or_default()
}

fn clip(text: &str, limit: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");

    if flat.chars().count() <= limit {
        return flat;
    }

    flat.chars().take(limit).collect()
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::thread;

    use cortex_core::{Message, StopReason, Usage};

    use super::*;

    fn call(id: &str, name: &str, input: Value) -> ToolCall {
        ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            input,
        }
    }

    fn result(id: &str, output: &str, is_error: bool) -> ToolResult {
        ToolResult {
            call_id: id.to_string(),
            output: output.to_string(),
            is_error,
            images: Vec::new(),
        }
    }

    /// A stand-in for the service: answers every request with `{"step": 7}`
    /// and keeps what it was sent.
    fn fake_service() -> (String, Arc<Mutex<Vec<(String, Value)>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();

        thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();

                reader.read_line(&mut request_line).unwrap();

                let mut length = 0;

                loop {
                    let mut header = String::new();

                    reader.read_line(&mut header).unwrap();

                    if header.trim().is_empty() {
                        break;
                    }

                    if let Some(value) = header.to_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap();
                    }
                }

                let mut body = vec![0; length];

                reader.read_exact(&mut body).unwrap();

                let path = request_line.split(' ').nth(1).unwrap().to_string();
                let body = serde_json::from_slice(&body).unwrap_or(Value::Null);

                log.lock().unwrap().push((path, body));

                let reply = r#"{"task": 3, "step": 7, "advice": "Mind: run the tests.", "kind": "ask",
                    "warning": "Mind: that failed before.", "text": "earlier work", "options": [],
                    "message": "commit it"}"#;

                write!(
                    stream,
                    "HTTP/1.0 200 OK\r\nContent-Length: {}\r\n\r\n{reply}",
                    reply.len()
                )
                .unwrap();
            }
        });

        (url, seen)
    }

    fn responded(calls: Vec<ToolCall>) -> Event {
        Event::ModelResponded {
            step: 1,
            message: Message {
                role: cortex_core::Role::Assistant,
                blocks: calls
                    .into_iter()
                    .map(cortex_core::Block::ToolCall)
                    .collect(),
            },
            stop_reason: StopReason::ToolUse,
            usage: Usage::default(),
            duration_ms: 0,
        }
    }

    #[test]
    fn the_next_request_is_suggested_and_then_learned_from() {
        let (url, seen) = fake_service();
        let mind = Mind::new(&url, "repo");

        // Nothing has ended yet, so there is nothing to go on.
        assert!(mind.suggest().is_none());

        mind.record(&started("fix the fee rounding"));
        mind.record(&responded(vec![call(
            "a",
            "edit_file",
            json!({ "path": "fees.py" }),
        )]));
        mind.record(&Event::ToolFinished {
            step: 1,
            tool: "edit_file".to_string(),
            result: result("a", "edited", false),
            duration_ms: 3,
        });
        mind.record(&finished(None));

        assert_eq!(mind.suggest().as_deref(), Some("commit it"));

        mind.record(&started("run the tests"));

        let seen = seen.lock().unwrap();
        let sent = |path: &str| {
            seen.iter()
                .find(|(seen, _)| seen == path)
                .unwrap()
                .1
                .clone()
        };

        let asked_for = sent("/suggest");

        assert_eq!(
            asked_for["ending"],
            "files were edited and nothing checked them | task: fix the fee rounding"
        );
        assert_eq!(asked_for["edited"], true);
        assert!(asked_for["checked"].is_null());

        let learned = sent("/asked");

        assert_eq!(learned["message"], "run the tests");
        assert_eq!(learned["shown"], "commit it");
        assert_eq!(learned["ending"], asked_for["ending"]);
    }

    #[test]
    fn a_call_is_described_by_its_tool_and_target() {
        let shell = call("1", "shell", json!({ "command": "cargo   test -p core" }));
        let edit = call(
            "2",
            "edit_file",
            json!({ "path": "src/lib.rs", "old_text": "a" }),
        );
        let bare = call("3", "list_files", json!({}));

        assert_eq!(
            describe(&shell),
            ("shell: cargo test -p core".to_string(), String::new())
        );
        assert_eq!(
            describe(&edit),
            (
                "edit_file: src/lib.rs".to_string(),
                "src/lib.rs".to_string()
            )
        );
        assert_eq!(describe(&bare).0, "list_files");
    }

    #[test]
    fn only_a_command_or_an_error_says_whether_a_call_worked() {
        let shell = |output: &str| verdict("shell", &result("1", output, false));

        assert_eq!(shell("exit code: 0\n\nstdout:\nok"), Some(Verdict::Worked));

        // A command that exits non-zero may have found out what it was run
        // to find out, so it is not yet a failure of the action.
        assert_eq!(
            shell("exit code: 101\n\nstderr:\nboom"),
            Some(Verdict::Exited)
        );
        assert_eq!(
            shell("exit code: none (killed by a signal)"),
            Some(Verdict::Exited)
        );

        // Output that happens to print an exit code is not the command's own.
        assert_eq!(
            shell("exit code: 1\n\nstdout:\nexit code: 0"),
            Some(Verdict::Exited)
        );

        assert_eq!(
            verdict("read_file", &result("1", "fn main() {}", false)),
            None
        );
        assert_eq!(
            verdict("read_file", &result("1", "no such file", true)),
            Some(Verdict::Failed)
        );
    }

    #[test]
    fn a_failure_is_summed_up_by_its_most_telling_line() {
        let output = "exit code: 101\n\nstderr:\n   Compiling core\nerror[E0308]: mismatched types";

        assert_eq!(
            summary(
                "shell: cargo test",
                Some(Verdict::Exited),
                &result("1", output, false)
            ),
            "`shell: cargo test` failed: error[E0308]: mismatched types"
        );
        assert_eq!(
            summary(
                "shell: ls",
                Some(Verdict::Exited),
                &result("1", "exit code: 2\n\nstderr:\nno such", false)
            ),
            "`shell: ls` failed: no such"
        );
        assert_eq!(
            summary(
                "shell: rm -rf build",
                Some(Verdict::Failed),
                &result("1", DECLINED, true)
            ),
            "`shell: rm -rf build` was declined by the person"
        );
    }

    #[test]
    fn an_unreachable_service_changes_nothing() {
        // Port 9 is the discard service; nothing answers HTTP there.
        let mind = Mind::new("http://127.0.0.1:9", "repo");

        assert!(mind.health().is_none());

        mind.record(&Event::RunStarted {
            instruction: "fix it".to_string(),
            provider: "test".to_string(),
            model: "test".to_string(),
        });

        assert!(mind.is_down());
        assert!(mind.advise(1, 10).is_none());
    }

    fn started(instruction: &str) -> Event {
        Event::RunStarted {
            instruction: instruction.to_string(),
            provider: "test".to_string(),
            model: "test".to_string(),
        }
    }

    fn finished(verified: Option<bool>) -> Event {
        Event::RunFinished {
            outcome: cortex_core::Outcome::Completed,
            steps: 2,
            usage: Usage {
                input_tokens: 900,
                output_tokens: 100,
                ..Usage::default()
            },
            duration_ms: 9,
            verified,
        }
    }

    fn tool_finished(id: &str, output: &str) -> Event {
        Event::ToolFinished {
            step: 1,
            tool: "shell".to_string(),
            result: result(id, output, false),
            duration_ms: 3,
        }
    }

    #[test]
    fn a_run_reaches_the_service_step_by_step() {
        let (url, seen) = fake_service();
        let mind = Mind::new(&url, "repo");

        mind.record(&started("fix the fee rounding"));
        mind.record(&responded(vec![call(
            "a",
            "shell",
            json!({ "command": "pytest -q" }),
        )]));
        mind.record(&tool_finished("a", "exit code: 1\n\nstdout:\n1 failed"));

        let advice = mind.advise(1, 10).unwrap();

        mind.record(&responded(Vec::new()));
        mind.record(&Event::VerificationRan {
            step: 2,
            passed: true,
            checks: Vec::new(),
            duration_ms: 5,
        });
        mind.record(&finished(Some(true)));

        // The failure it predicted, and then what it had to say.
        assert_eq!(
            advice.text,
            "Mind: that failed before.\nMind: run the tests."
        );

        let seen = seen.lock().unwrap();
        let paths: Vec<&str> = seen.iter().map(|(path, _)| path.as_str()).collect();

        assert_eq!(
            paths,
            [
                "/begin", "/step", "/result", "/advise", "/step", "/result", "/end"
            ]
        );
        assert_eq!(seen[0].1["arm"], "advised");
        assert_eq!(seen[0].1["several"], true);
        assert_eq!(seen[1].1["task"], 3);
        assert_eq!(
            seen[1].1["situation"],
            "at the start | task: fix the fee rounding"
        );
        assert_eq!(seen[1].1["action"], "shell: pytest -q");
        assert_eq!(seen[2].1["outcome"], 0.0);
        assert_eq!(seen[2].1["soft"], true);
        assert_eq!(
            seen[3].1["situation"],
            "after `shell: pytest -q` failed: 1 failed | task: fix the fee rounding"
        );
        assert_eq!(seen[4].1["action"], "finish");
        assert_eq!(seen[5].1["outcome"], 1.0);
        assert_eq!(seen[6].1["outcome"], 1.0);
        assert_eq!(seen[6].1["n_steps"], 2);
        assert_eq!(seen[6].1["tokens_in"], 900);
        assert_eq!(seen[6].1["tokens_out"], 100);
        assert_eq!(seen[6].1["advised"], 2);
    }

    #[test]
    fn what_the_person_says_reaches_the_brain_during_a_run_and_after_it() {
        let (url, seen) = fake_service();
        let mind = Mind::new(&url, "repo");

        mind.record(&started("fetch the prices"));
        mind.record(&Event::UserInterjected {
            step: 1,
            text: "no, use urllib".to_string(),
        });
        mind.record(&finished(None));
        mind.record(&started("that broke the totals"));

        let seen = seen.lock().unwrap();
        let feedback: Vec<&Value> = seen
            .iter()
            .filter(|(path, _)| path == "/feedback")
            .map(|(_, body)| body)
            .collect();

        assert_eq!(feedback.len(), 2);
        assert_eq!(feedback[0]["text"], "no, use urllib");
        assert_eq!(feedback[0]["after"], Value::Null);

        // The next instruction is heard as a verdict on the run before it.
        assert_eq!(feedback[1]["task"], 3);
        assert_eq!(feedback[1]["text"], "that broke the totals");
        assert_eq!(feedback[1]["after"], true);
    }

    #[test]
    fn a_question_is_asked_once_and_no_line_is_said_twice() {
        let (url, _seen) = fake_service();
        let mind = Mind::new(&url, "repo");

        mind.record(&started("fix it"));

        assert_eq!(mind.advise(1, 10).unwrap().text, "Mind: run the tests.");
        assert!(mind.advise(2, 10).is_none());
    }

    #[test]
    fn a_control_run_is_watched_and_told_nothing() {
        let (url, seen) = fake_service();
        let mind = Mind::new(&url, "repo").with_control(1.0);

        mind.record(&started("fix it"));
        mind.record(&responded(vec![call(
            "a",
            "shell",
            json!({ "command": "ls" }),
        )]));
        mind.record(&tool_finished("a", "exit code: 0"));

        assert!(mind.advise(1, 10).is_none());
        assert!(mind.inspect(&json!({ "imagine": ["shell: ls"] })).is_none());

        let seen = seen.lock().unwrap();
        let paths: Vec<&str> = seen.iter().map(|(path, _)| path.as_str()).collect();

        assert_eq!(paths, ["/begin", "/step", "/result"]);
        assert_eq!(seen[0].1["arm"], "control");
    }

    #[test]
    fn the_model_can_have_actions_imagined() {
        let (url, seen) = fake_service();
        let mind = Mind::new(&url, "repo");

        mind.record(&started("fix it"));

        let imagined = mind
            .inspect(&json!({ "imagine": ["shell: cargo test", "finish"] }))
            .unwrap();

        assert_eq!(imagined["source"], "mind");
        assert!(mind.inspect(&json!({ "neuron_ids": [1] })).is_none());

        let seen = seen.lock().unwrap();

        assert_eq!(seen[1].0, "/imagine");
        assert_eq!(seen[1].1["actions"], json!(["shell: cargo test", "finish"]));
        assert_eq!(seen[1].1["situation"], "at the start | task: fix it");
    }
}
