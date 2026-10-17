//! A record of where a task stands, kept beside its log.
//!
//! A session's log is the full story of a run, and reading it back gives
//! the conversation again (see [`crate::replay`]). This is the short
//! version, written after every step: the instruction, the conversation
//! as it stands, the plan, the steps and tokens so far, and whether the
//! run finished. A task that was cut off, because its process died or
//! the machine went to sleep, is taken up from this record with
//! [`TaskState::progress`] and [`crate::Agent::resume`], with its counts
//! intact, without reading the log at all.
//!
//! The record is replaced as a whole, through a temporary file and a
//! rename, so a crash leaves either the old record or the new one and
//! never half of one.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::agent::{Outcome, Progress, close_unanswered_calls};
use crate::compact;
use crate::message::Message;
use crate::plan::{self, Plan};
use crate::replay;
use crate::trajectory::{Event, TrajectorySink, unix_millis};
use crate::usage::Usage;
use crate::verify::Verification;

/// Bumped when the record changes in a way older readers cannot follow.
pub const VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Steps are still being taken, or were when the record was last
    /// written: a record left like this belongs to a run that was cut off.
    Running,

    /// The run came to an end of its own.
    Finished,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskState {
    pub version: u32,

    pub instruction: String,

    pub status: TaskStatus,

    /// Model calls answered so far.
    pub steps: u32,

    pub usage: Usage,

    /// Time spent working, over every stretch of the run.
    pub elapsed_ms: u64,

    /// Why the run stopped, once it has.
    #[serde(default)]
    pub outcome: Option<Outcome>,

    /// The latest plan the model wrote with `update_plan`.
    #[serde(default)]
    pub plan: Option<Plan>,

    /// The conversation as the next request would send it.
    pub conversation: Vec<Message>,

    pub updated_ms: u64,
}

impl TaskState {
    /// The record at `path`. `None` when there is none, or it cannot be
    /// read by this version.
    pub fn load(path: &Path) -> Option<Self> {
        let text = fs::read_to_string(path).ok()?;
        let state: Self = serde_json::from_str(&text).ok()?;

        (state.version == VERSION).then_some(state)
    }

    /// Writes the record in place of the one at `path`, whole.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        write_atomically(path, &serde_json::to_vec(self)?)
    }

    /// Whether the run can be carried on: it was cut off part-way, and
    /// has a conversation to carry on.
    pub fn can_resume(&self) -> bool {
        self.status == TaskStatus::Running && !self.conversation.is_empty()
    }

    /// The conversation to carry on with: the calls the model asked for
    /// and never heard back from are answered as not run, since a
    /// provider rejects a call left without an answer.
    pub fn resumable_conversation(&self) -> Vec<Message> {
        let mut conversation = self.conversation.clone();

        close_unanswered_calls(&mut conversation);

        conversation
    }

    /// How far the run had got, for [`crate::Agent::resume`].
    pub fn progress(&self) -> Progress {
        let mut progress = Progress::starting(&self.instruction);

        progress.steps = self.steps;
        progress.usage = self.usage;
        progress.elapsed = Duration::from_millis(self.elapsed_ms);

        progress
    }
}

/// Keeps a [`TaskState`] up to date from a run's events.
pub struct TaskStateSink {
    path: PathBuf,
    inner: Mutex<Inner>,
}

struct Inner {
    /// The conversation before this stretch of the run began: empty for
    /// a new run, what the record held for one taken up again.
    prior: Vec<Message>,

    /// This stretch's events since `prior`, from which the rest of the
    /// conversation is rebuilt.
    events: Vec<Event>,

    state: TaskState,

    /// Counts as they were when this stretch began.
    usage_so_far: Usage,
    prior_elapsed: Duration,
    began: Instant,
}

impl TaskStateSink {
    /// A record for a new run of `instruction`, at `path`.
    pub fn new(path: impl Into<PathBuf>, instruction: &str) -> Self {
        let state = TaskState {
            version: VERSION,
            instruction: instruction.to_string(),
            status: TaskStatus::Running,
            steps: 0,
            usage: Usage::default(),
            elapsed_ms: 0,
            outcome: None,
            plan: None,
            conversation: Vec::new(),
            updated_ms: now(),
        };

        Self::from_state(path.into(), state, Vec::new())
    }

    /// A record that carries on from `state`, whose run is about to be
    /// resumed with `conversation` (from
    /// [`TaskState::resumable_conversation`]).
    pub fn resuming(
        path: impl Into<PathBuf>,
        mut state: TaskState,
        conversation: Vec<Message>,
    ) -> Self {
        state.status = TaskStatus::Running;
        state.outcome = None;
        state.conversation = conversation.clone();

        Self::from_state(path.into(), state, conversation)
    }

    fn from_state(path: PathBuf, state: TaskState, prior: Vec<Message>) -> Self {
        let inner = Inner {
            prior,
            events: Vec::new(),
            usage_so_far: state.usage,
            prior_elapsed: Duration::from_millis(state.elapsed_ms),
            began: Instant::now(),
            state,
        };

        let sink = Self {
            path,
            inner: Mutex::new(inner),
        };

        sink.save(&sink.lock());

        sink
    }

    /// The record as it stands.
    pub fn state(&self) -> TaskState {
        self.lock().state.clone()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn save(&self, inner: &Inner) {
        if let Err(error) = inner.state.save(&self.path) {
            eprintln!(
                "warning: the task record {} could not be written: {error}",
                self.path.display()
            );
        }
    }
}

impl Inner {
    fn conversation(&self) -> Vec<Message> {
        let mut conversation = self.prior.clone();

        conversation.extend(replay::conversation(&self.events));

        conversation
    }

    /// Folds this stretch's events into `prior`, as the conversation
    /// stood just before the next request, so that a compaction can be
    /// applied to all of it.
    fn fold(&mut self) {
        let feedback = pending_feedback(&self.events);

        self.prior = self.conversation();
        self.events.clear();

        if let Some(feedback) = feedback {
            self.prior.push(Message::user_text(feedback));
        }
    }

    fn refresh(&mut self) {
        self.state.conversation = self.conversation();
        self.state.elapsed_ms = (self.prior_elapsed + self.began.elapsed()).as_millis() as u64;
        self.state.updated_ms = now();
    }
}

impl TrajectorySink for TaskStateSink {
    fn record(&self, event: &Event) {
        let mut inner = self.lock();

        let save = match event {
            Event::ModelStarted { .. } => true,

            Event::ModelResponded {
                step,
                message,
                usage,
                ..
            } => {
                inner.state.steps = *step;
                inner.usage_so_far += *usage;
                inner.state.usage = inner.usage_so_far;

                let planned = message
                    .tool_calls()
                    .into_iter()
                    .rev()
                    .find(|call| call.name == plan::TOOL)
                    .and_then(|call| Plan::from_input(&call.input));

                if let Some(planned) = planned {
                    inner.state.plan = Some(planned);
                }

                true
            }

            Event::ContextCompacted { .. } => true,

            Event::RunFinished {
                outcome,
                steps,
                usage,
                ..
            } => {
                inner.state.status = TaskStatus::Finished;
                inner.state.outcome = Some(*outcome);
                inner.state.steps = *steps;
                inner.state.usage = *usage;

                true
            }

            _ => false,
        };

        match event {
            // Replaying a compaction over only this stretch would lose
            // what came before it, so it is applied to the whole.
            Event::ContextCompacted { kept, text, .. } => {
                inner.fold();

                compact::apply(&mut inner.prior, *kept, text);
            }

            _ => inner.events.push(event.clone()),
        }

        if save {
            inner.refresh();
            self.save(&inner);
        }
    }
}

/// What the checks or the review sent back after the last response, and
/// that the next request would carry, as [`crate::replay`] reads it.
fn pending_feedback(events: &[Event]) -> Option<String> {
    let mut feedback = None;

    for event in events {
        match event {
            Event::ModelResponded { .. }
            | Event::RunStarted { .. }
            | Event::RunFinished { .. } => feedback = None,

            Event::VerificationRan { passed, checks, .. } => {
                feedback = (!passed).then(|| {
                    Verification {
                        checks: checks.clone(),
                    }
                    .feedback()
                });
            }

            Event::ReviewRan {
                feedback: Some(said),
                ..
            } => feedback = Some(said.clone()),

            _ => {}
        }
    }

    feedback
}

fn now() -> u64 {
    unix_millis() as u64
}

/// Replaces the file at `path` with `bytes` through a temporary file in
/// the same folder, so that a reader sees the old file or the new one.
pub fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let folder = path.parent().unwrap_or(Path::new("."));

    fs::create_dir_all(folder)?;

    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "record".to_string());

    let temporary = folder.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        unique()
    ));

    fs::write(&temporary, bytes)?;

    fs::rename(&temporary, path).inspect_err(|_| {
        let _ = fs::remove_file(&temporary);
    })
}

/// A number no other call in this process gets, for temporary names.
fn unique() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    NEXT.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::message::{Block, Role, ToolCall, ToolResult};
    use crate::provider::StopReason;

    fn started(instruction: &str) -> Event {
        Event::RunStarted {
            instruction: instruction.to_string(),
            provider: "scripted".to_string(),
            model: "scripted".to_string(),
        }
    }

    fn calls(step: u32, id: &str, name: &str, input: serde_json::Value) -> Event {
        Event::ModelResponded {
            step,
            message: Message {
                role: Role::Assistant,
                blocks: vec![Block::ToolCall(ToolCall {
                    id: id.to_string(),
                    name: name.to_string(),
                    input,
                })],
            },
            stop_reason: StopReason::ToolUse,
            usage: Usage {
                input_tokens: 10,
                output_tokens: 5,
                ..Usage::default()
            },
            duration_ms: 1,
        }
    }

    fn finished(id: &str) -> Event {
        Event::ToolFinished {
            step: 1,
            tool: "read_file".to_string(),
            result: ToolResult {
                call_id: id.to_string(),
                output: "fn main() {}".to_string(),
                is_error: false,
                images: Vec::new(),
            },
            duration_ms: 1,
        }
    }

    #[test]
    fn the_record_follows_the_run_and_survives_a_cut() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("state.json");

        let sink = TaskStateSink::new(&path, "fix it");

        sink.record(&started("fix it"));
        sink.record(&Event::ModelStarted { step: 1 });
        sink.record(&calls(
            1,
            "plan",
            plan::TOOL,
            json!({ "steps": [{ "step": "Read", "status": "in_progress" }] }),
        ));
        sink.record(&Event::ModelStarted { step: 2 });
        sink.record(&calls(2, "a", "read_file", json!({ "path": "a.rs" })));
        sink.record(&finished("a"));

        // The process dies here, before step 3 is asked.
        let state = TaskState::load(&path).unwrap();

        assert!(state.can_resume());
        assert_eq!(state.steps, 2);
        assert_eq!(state.usage.output_tokens, 10);
        assert_eq!(state.plan.as_ref().unwrap().steps[0].step, "Read");

        // The call whose result was not saved is answered as not run.
        let conversation = state.resumable_conversation();
        let last = conversation.last().unwrap();

        assert_eq!(last.role, Role::User);
        assert!(matches!(&last.blocks[0], Block::ToolResult(result) if result.is_error));

        let progress = state.progress();

        assert_eq!(progress.steps, 2);
        assert_eq!(progress.usage.input_tokens, 20);

        // Taken up again, the record goes on from where it was.
        let resumed = TaskStateSink::resuming(&path, state, conversation.clone());

        resumed.record(&Event::RunResumed { steps: 2 });
        resumed.record(&Event::ModelStarted { step: 3 });
        resumed.record(&Event::ModelResponded {
            step: 3,
            message: Message {
                role: Role::Assistant,
                blocks: vec![Block::Text {
                    text: "Done.".to_string(),
                }],
            },
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
            duration_ms: 1,
        });
        resumed.record(&Event::RunFinished {
            outcome: Outcome::Completed,
            steps: 3,
            usage: Usage {
                input_tokens: 20,
                output_tokens: 10,
                ..Usage::default()
            },
            duration_ms: 5,
            verified: None,
        });

        let done = TaskState::load(&path).unwrap();

        assert!(!done.can_resume());
        assert_eq!(done.outcome, Some(Outcome::Completed));
        assert_eq!(done.conversation.len(), conversation.len() + 1);
        assert_eq!(done.conversation[..conversation.len()], conversation[..]);
    }

    #[test]
    fn a_record_from_another_version_is_not_read() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("state.json");

        let sink = TaskStateSink::new(&path, "x");
        let mut state = sink.state();

        state.version = VERSION + 1;
        state.save(&path).unwrap();

        assert!(TaskState::load(&path).is_none());
        assert!(TaskState::load(&folder.path().join("missing.json")).is_none());
    }
}
