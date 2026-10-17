//! A record of everything that happens in a run.
//!
//! Trajectories are the debugging log, the source for evaluation metrics,
//! and later the training data for the learned components. The console UI
//! is also just a sink.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::Outcome;
use crate::message::{Image, Message, ToolCall, ToolResult};
use crate::provider::StopReason;
use crate::redact;
use crate::reflect::Lesson;
use crate::review::Requirement;
use crate::usage::Usage;
use crate::verify::CheckOutcome;
use crate::window::{ContextUse, PlanQuota};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    #[serde(alias = "brain_observed")]
    AdviserObserved {
        source: String,
        step: u32,
        detail: Value,
    },
    RunStarted {
        instruction: String,
        provider: String,
        model: String,
    },

    /// Pictures sent with the instruction of the run that has just
    /// started.
    ImagesAttached {
        images: Vec<Image>,
    },

    /// A run that stopped on a provider failure is being carried on from
    /// the step that failed. `steps` is how many it had finished.
    RunResumed {
        steps: u32,
    },

    /// Context placed in front of the instruction (memory, code found by
    /// the router). `detail` records how it was chosen.
    ContextPrepared {
        source: String,
        text: String,
        detail: Value,
        duration_ms: u64,
    },

    /// A request has been sent and the model is working on it.
    ModelStarted {
        step: u32,
    },

    ModelResponded {
        step: u32,
        message: Message,
        stop_reason: StopReason,
        usage: Usage,

        /// Time from sending the request to having the whole response.
        /// Absent in logs written before this field existed.
        #[serde(default)]
        duration_ms: u64,
    },

    ToolStarted {
        step: u32,
        call: ToolCall,
    },

    ToolFinished {
        step: u32,
        tool: String,
        result: ToolResult,
        duration_ms: u64,
    },

    /// The person said something while the run was in progress, and it
    /// was shown to the model after `step`.
    UserInterjected {
        step: u32,
        text: String,
    },

    ContextPruned {
        step: u32,
        results_trimmed: usize,
        chars_removed: usize,
    },

    /// Tool output older than the last few steps was replaced with one
    /// line each.
    ContextMasked {
        step: u32,
        results_masked: usize,
        chars_removed: usize,
    },

    /// The older part of the conversation was replaced by a summary the
    /// model wrote. Everything but the last `kept` messages went, and
    /// `text` is the message now in its place, so the log replays to the
    /// same conversation.
    ContextCompacted {
        /// The step about to be asked, or 0 for one made between runs.
        step: u32,

        kept: usize,
        text: String,
        messages_replaced: usize,

        /// Characters of the conversation before and after.
        chars_before: usize,
        chars_after: usize,

        /// What the summary call used. In a run it is part of the run's
        /// total; between runs it is in no run's total.
        usage: Usage,

        duration_ms: u64,

        /// It was done because the window was filling up, not because
        /// the person asked.
        automatic: bool,
    },

    /// How full the context window was at the model call of `step`, and,
    /// from a provider that says so, how much of a subscription's
    /// allowance is used.
    ContextMeasured {
        step: u32,

        /// Boxed so that the largest event stays small.
        context: Box<ContextUse>,

        #[serde(default)]
        quota: Option<PlanQuota>,
    },

    /// Which advisers stay silent in the run that has just started, so
    /// that runs with and without each can be compared. Whether a source
    /// is held out follows from `seed` alone
    /// ([`crate::effect::held_out`]); the lists name those known when the
    /// run began.
    ExperimentAssigned {
        seed: u64,

        /// The share of runs an adviser is held out of.
        share: f64,

        /// The same for briefings; 0 when none are held out.
        briefing_share: f64,

        /// Advisers held out of this run.
        held_out: Vec<String>,

        /// Advisers free to speak in it.
        speaking: Vec<String>,

        /// The briefings shown. One that was held out has its own
        /// `ContextWithheld` event before this.
        briefed: Vec<String>,
    },

    /// What an adviser would have said after `step`, or a briefing would
    /// have put before the task (`step` 0), in a run it is held out of.
    /// The model was not shown it.
    ContextWithheld {
        step: u32,
        source: String,
        text: String,
        detail: Value,
        duration_ms: u64,
    },

    /// The finished work was held against the request. `feedback` is what
    /// was sent back to the model, when something looked unmet.
    ReviewRan {
        step: u32,
        requirements: Vec<Requirement>,
        feedback: Option<String>,
        usage: Usage,
        duration_ms: u64,
    },

    /// What the model said the run taught, for memory to keep.
    LessonsLearned {
        lessons: Vec<Lesson>,
        usage: Usage,
        duration_ms: u64,
    },

    /// One of several attempts at the task has started in its own copy
    /// of the project.
    AttemptStarted {
        attempt: u32,
        of: u32,
        strategy: String,
    },

    /// An attempt ended. `review_score` is the share of the request's
    /// requirements its changes meet, as judged by the model.
    AttemptFinished {
        attempt: u32,
        strategy: String,
        outcome: Option<Outcome>,
        steps: u32,
        usage: Usage,
        verified: Option<bool>,
        review_score: Option<f64>,
        changed_files: usize,
        error: Option<String>,
        duration_ms: u64,
    },

    /// The attempt whose changes were kept. Its steps follow in the log
    /// as if it had been the only one.
    AttemptChosen {
        attempt: u32,
        of: u32,
        reason: String,
    },

    /// The verification gate ran the project's checks on the finished work.
    VerificationRan {
        step: u32,
        passed: bool,
        checks: Vec<CheckOutcome>,
        duration_ms: u64,
    },

    /// The project was saved before the run's first change, as the
    /// checkpoint `id`, so the run can be undone. `error` says why it
    /// could not be, in which case `id` is empty.
    CheckpointSaved {
        step: u32,
        id: String,
        label: String,
        #[serde(default)]
        error: Option<String>,
    },

    /// The person allowed or refused a call that asked first.
    ApprovalDecided {
        step: u32,
        tool: String,
        summary: String,
        approved: bool,
    },

    /// The run moved to a stronger model after repeated failures.
    Escalated {
        step: u32,
        from: String,
        to: String,
        reason: String,
    },

    /// The run's spending crossed a line of its budget: `share` is how
    /// much of it is used (1.0 is all of it). `cost` is in US dollars,
    /// when the model's price is known.
    BudgetReached {
        step: u32,
        share: f64,
        tokens: u64,
        #[serde(default)]
        cost: Option<f64>,
    },

    RunFinished {
        outcome: Outcome,
        steps: u32,
        usage: Usage,
        duration_ms: u64,
        verified: Option<bool>,
    },
}

pub trait TrajectorySink: Send + Sync {
    fn record(&self, event: &Event);
}

/// Appends one JSON object per event to a file.
pub struct JsonlSink {
    file: Mutex<File>,
}

#[derive(Serialize)]
struct Line<'a> {
    ts_ms: u128,

    #[serde(flatten)]
    event: &'a Event,
}

impl JsonlSink {
    pub fn create(path: &Path) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let file = File::create(path)?;

        Ok(Self {
            file: Mutex::new(file),
        })
    }

    /// Opens an existing log to add to it, as when a session is taken up
    /// again.
    pub fn append(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new().append(true).open(path)?;

        Ok(Self {
            file: Mutex::new(file),
        })
    }

    fn write_line(&self, event: &Event) -> io::Result<()> {
        let line = Line {
            ts_ms: unix_millis(),
            event,
        };

        let json = serde_json::to_string(&line)?;
        let json = redact::redact(&json);

        let mut file = self
            .file
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        writeln!(file, "{json}")
    }
}

impl TrajectorySink for JsonlSink {
    fn record(&self, event: &Event) {
        if let Err(error) = self.write_line(event) {
            eprintln!("warning: could not write trajectory event: {error}");
        }
    }
}

pub fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or_default()
}
