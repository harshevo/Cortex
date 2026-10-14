//! Finding out whether the advisers help.
//!
//! An adviser adds a line for the model between steps, and a context
//! source puts a briefing before the task. Whether runs go better for it
//! cannot be seen by watching the advice: that the advised step came next
//! says nothing about whether the run would have gone the same way
//! unadvised. The only honest measure is a controlled comparison, and
//! this module builds one into the engine.
//!
//! **Hold-outs.** When a run starts, a seed is drawn and recorded. Each
//! adviser is held out of the run with a set probability, decided from
//! the seed and the adviser's name alone ([`held_out`]), each adviser on
//! its own. A held-out adviser still watches the run and learns from it,
//! and is still asked for its advice; what it would have said is logged
//! ([`Event::ContextWithheld`]) and not shown to the model. Briefings can
//! be held out the same way, which is off unless asked for, because a run
//! without its memory differs more.
//!
//! **Records.** [`EffectSink`] writes one line per run to an
//! [`EffectLedger`]: which advisers spoke and which were held out, how
//! often each advised or would have, and how the run went.
//!
//! **Analysis.** [`analyse`] compares, for each adviser, the runs in
//! which it spoke with the runs in which it was held out, among the runs
//! in which it had something to say at all. Because chance alone decided
//! which was which, a difference between the two groups that is larger
//! than chance explains is the adviser's doing.
//!
//! Three cautions for whoever reads the numbers:
//!
//! - Below [`MIN_PER_ARM`] runs on either side the verdict is
//!   [`Verdict::TooFewRuns`], whatever the difference looks like.
//! - Every adviser is compared on every metric. At a 95% interval one
//!   comparison in twenty shows a difference by chance alone, so among
//!   many, some "helps" and "hurts" will be chance.
//!   [`EffectReport::comparisons`] says how many were made.
//! - The numbers named `observational` (how often the model did what was
//!   advised, and how runs went when it did) are not a comparison of
//!   this kind. A model follows advice more readily on an easy task, so
//!   they say nothing about cause.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::Outcome;
use crate::ledger::{self, DAY_MS, Months};
use crate::message::Message;
use crate::provider::StopReason;
use crate::trajectory::{Event, TrajectorySink, unix_millis};
use crate::usage::Usage;

/// The share of runs an adviser is held out of unless another is given.
pub const DEFAULT_SHARE: f64 = 0.25;

/// Runs needed on each side of a comparison before it is given a
/// verdict.
pub const MIN_PER_ARM: u64 = 20;

/// Briefings that are never held out: without them the run would not be
/// the one that was asked for.
const ALWAYS_SHOWN: [&str; 2] = ["skill", "rollout"];

/// Whether `provider` is a stand-in for a model, whose runs measure
/// nothing.
pub fn is_stand_in(provider: &str) -> bool {
    ledger::STAND_INS.contains(&provider)
}

/// How the experiment is set up.
#[derive(Debug, Clone, PartialEq)]
pub struct ExperimentConfig {
    /// The share of runs each adviser is held out of, from 0 to 1. With
    /// 0 no adviser is held out.
    pub share: f64,

    /// The same for briefings. 0 unless asked for.
    pub briefing_share: f64,

    /// Use this seed for every run instead of drawing one. For tests.
    pub seed: Option<u64>,

    /// Also experiment on runs of the stand-in models that tests and the
    /// demonstration use.
    pub stand_ins: bool,
}

impl Default for ExperimentConfig {
    fn default() -> Self {
        Self {
            share: DEFAULT_SHARE,
            briefing_share: 0.0,
            seed: None,
            stand_ins: false,
        }
    }
}

/// The experiment as it applies to one run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Experiment {
    pub seed: u64,
    pub share: f64,
    pub briefing_share: f64,
}

static DRAWN: AtomicU64 = AtomicU64::new(0);

impl Experiment {
    /// The experiment for a run that is starting. `None` when nothing
    /// would be held out.
    pub fn draw(config: &ExperimentConfig) -> Option<Self> {
        let share = config.share.clamp(0.0, 1.0);
        let briefing_share = config.briefing_share.clamp(0.0, 1.0);

        if share <= 0.0 && briefing_share <= 0.0 {
            return None;
        }

        // The time, the process and a count: different for every run on
        // this machine, which is all a seed has to be.
        let seed = config.seed.unwrap_or_else(|| {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos() as u64)
                .unwrap_or_default();

            mix(nanos
                ^ (u64::from(std::process::id()) << 32)
                ^ DRAWN.fetch_add(1, Ordering::Relaxed))
        });

        Some(Self {
            seed,
            share,
            briefing_share,
        })
    }

    /// Whether what the adviser named `source` says is kept from the
    /// model in this run.
    pub fn withholds_advice(&self, source: &str) -> bool {
        held_out(self.seed, source, self.share)
    }

    /// Whether the briefing from `source` is left out of this run.
    pub fn withholds_briefing(&self, source: &str) -> bool {
        !ALWAYS_SHOWN.contains(&source)
            && held_out(
                self.seed,
                &format!("briefing: {source}"),
                self.briefing_share,
            )
    }
}

/// Whether the run with this `seed` holds out `source`, when a source is
/// held out of `share` of all runs. The same seed and name always give
/// the same answer, and different names are decided independently.
pub fn held_out(seed: u64, source: &str, share: f64) -> bool {
    if share <= 0.0 {
        return false;
    }

    // FNV-1a over the name, then mixed with the seed.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;

    for byte in source.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }

    let drawn = mix(seed ^ mix(hash));

    // The top 53 bits as a number from 0 up to, not including, 1.
    ((drawn >> 11) as f64 / (1u64 << 53) as f64) < share
}

/// SplitMix64's finishing step: every bit of the input moves every bit
/// of the output.
fn mix(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);

    value ^ (value >> 31)
}

/// How the person took a run, going by the first word of what they said
/// next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reaction {
    Accepted,
    Rejected,
}

/// Whether the person is turning down or approving what was done. The
/// same first-word test, with the same words, as the dendritic network
/// uses to hear the person. Most messages are neither.
pub fn reaction(text: &str) -> Option<Reaction> {
    const REJECTS: &[&str] = &[
        "no", "nope", "stop", "wait", "wrong", "don't", "dont", "revert", "undo", "not",
    ];

    const ACCEPTS: &[&str] = &[
        "yes", "yep", "ok", "okay", "good", "great", "nice", "perfect", "thanks", "correct",
        "exactly",
    ];

    let text = text.trim().to_lowercase();

    let first = text
        .split(|character: char| !character.is_alphanumeric() && character != '\'')
        .next()
        .unwrap_or_default();

    if REJECTS.contains(&first) || text.contains("that's wrong") || text.contains("still broken") {
        Some(Reaction::Rejected)
    } else if ACCEPTS.contains(&first) || text.contains("looks good") || text.contains("works now")
    {
        Some(Reaction::Accepted)
    } else {
        None
    }
}

/// What a piece of advice says the model should do next, where the
/// advice names something that can be checked against what it did.
#[derive(Debug, Clone, PartialEq)]
enum Expected {
    /// One of the controller's kinds of action: inspect, edit, run,
    /// revert, finish.
    Kind(String),

    /// Call this tool.
    Tool(String),

    /// Run a command that begins like this.
    Command(String),

    /// Work on one of these files.
    Files(Vec<String>),

    /// Do not do this next: the same, turned round.
    Not(Box<Expected>),
}

/// Reads what the advice expects out of the adviser's own record of it.
///
/// - The decision controllers give the kind of action as `advised`.
/// - The dendritic network gives a `label` such as `does: read_file`,
///   `runs: npm test` or `file: src/a.rs`, and a `kind` that says
///   whether it expects that next or warns against it.
/// - The intuition gives `hunches`; those of kind `file` name files.
///
/// Anything else, the mind's advice among it, names nothing comparable.
fn expectation(detail: &Value) -> Option<Expected> {
    if let Some(kind) = detail["advised"].as_str() {
        return Some(Expected::Kind(kind.to_string()));
    }

    if let Some(label) = detail["label"].as_str() {
        let (what, of) = label.split_once(": ")?;
        let of = of.trim().to_string();

        let expected = match what {
            "does" => Expected::Tool(of),
            "runs" => Expected::Command(of),
            "file" => Expected::Files(vec![of]),
            _ => return None,
        };

        return Some(match detail["kind"].as_str() {
            Some("warning") => Expected::Not(Box::new(expected)),
            _ => expected,
        });
    }

    let files: Vec<String> = detail["hunches"]
        .as_array()?
        .iter()
        .filter(|hunch| hunch["kind"] == "file")
        .filter_map(|hunch| hunch["text"].as_str())
        .map(str::to_string)
        .collect();

    (!files.is_empty()).then_some(Expected::Files(files))
}

/// Whether the model's next response did what was expected.
fn met(expected: &Expected, message: &Message, stop_reason: &StopReason) -> bool {
    let calls = message.tool_calls();

    let command = |call: &crate::message::ToolCall| {
        call.input
            .get("command")
            .and_then(Value::as_str)
            .map(|command| command.split_whitespace().collect::<Vec<_>>().join(" "))
    };

    match expected {
        Expected::Kind(kind) => kind_of(message, stop_reason) == kind,

        Expected::Tool(tool) => calls.iter().any(|call| call.name == *tool),

        Expected::Command(start) => calls
            .iter()
            .filter_map(command)
            .any(|command| command.starts_with(start.as_str())),

        Expected::Files(files) => calls.iter().any(|call| {
            let input = call.input.to_string();

            files.iter().any(|file| input.contains(file.as_str()))
        }),

        Expected::Not(expected) => !met(expected, message, stop_reason),
    }
}

/// The kind of action a response amounts to, as the decision controllers
/// sort them: the most consequential of its calls.
fn kind_of(message: &Message, stop_reason: &StopReason) -> &'static str {
    let calls = message.tool_calls();

    if calls.is_empty() || *stop_reason != StopReason::ToolUse {
        return "finish";
    }

    let kind = |call: &crate::message::ToolCall| match call.name.as_str() {
        "edit_file" | "write_file" => "edit",

        "shell" => {
            let command = call
                .input
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or_default();

            let reverts = ["git checkout", "git restore", "git stash", "git reset"];

            if reverts.iter().any(|revert| command.contains(revert)) {
                "revert"
            } else {
                "run"
            }
        }

        _ => "inspect",
    };

    let kinds: Vec<&str> = calls.iter().map(kind).collect();

    ["revert", "edit", "run"]
        .into_iter()
        .find(|kind| kinds.contains(kind))
        .unwrap_or("inspect")
}

/// What an adviser did in a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Arm {
    /// It advised and the model was shown it.
    Spoke,

    /// It was held out: what it said, if anything, was not shown.
    HeldOut,

    /// It was free to speak and had nothing to say.
    NothingToSay,
}

/// One adviser, or one source of briefings, in one run.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SourceRun {
    /// The name it signs with.
    pub source: String,

    /// The run was one it was held out of.
    pub held_out: bool,

    /// Times it advised, or would have had it not been held out.
    pub advised: u32,

    /// Of those, the times the advice named a next action that can be
    /// checked against what the model did.
    #[serde(default)]
    pub comparable: u32,

    /// Of those, the times the model's next response did it.
    #[serde(default)]
    pub followed: u32,
}

impl SourceRun {
    pub fn arm(&self) -> Arm {
        match (self.held_out, self.advised) {
            (true, _) => Arm::HeldOut,
            (false, 0) => Arm::NothingToSay,
            (false, _) => Arm::Spoke,
        }
    }
}

/// How a run came to an end.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ending {
    /// It reported its result.
    #[default]
    Finished,

    /// The person stopped it.
    Stopped,

    /// It did not report a result and nobody said why: the provider
    /// failed and it was not carried on, or the app was closed.
    Unfinished,
}

/// How long the instruction was.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskSize {
    /// Under 200 characters.
    #[default]
    Short,

    /// Under 1,000.
    Medium,

    Long,
}

impl TaskSize {
    pub fn of(instruction_chars: u64) -> Self {
        match instruction_chars {
            0..200 => Self::Short,
            200..1_000 => Self::Medium,
            _ => Self::Long,
        }
    }
}

/// One run and how it went.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RunRecord {
    /// When the run started, in milliseconds since 1970.
    pub ts_ms: u64,

    /// Names the run, for the line that later says how the person took
    /// it.
    pub run: String,

    pub session: String,

    /// The project or place the session worked in.
    pub place: String,

    pub provider: String,
    pub model: String,

    pub instruction_chars: u64,
    pub task_size: TaskSize,

    /// The seed the hold-outs follow from. `None` when the run was not
    /// part of the experiment.
    pub seed: Option<u64>,

    /// The share of runs an adviser was held out of.
    #[serde(default)]
    pub share: f64,

    pub advisers: Vec<SourceRun>,

    /// The sources of briefings. For these `advised` is 1.
    #[serde(default)]
    pub briefings: Vec<SourceRun>,

    pub ending: Ending,

    /// Why the model stopped. `None` for a run that did not finish.
    pub outcome: Option<Outcome>,

    /// Whether the project's checks passed on the finished work. `None`
    /// when they were not run.
    pub verified: Option<bool>,

    pub steps: u32,
    pub tool_calls: u32,
    pub failed_tool_calls: u32,

    /// Times the project's checks were run.
    pub verification_rounds: u32,

    #[serde(flatten)]
    pub usage: Usage,

    pub duration_ms: u64,

    /// Things the person said while the run was going.
    #[serde(default)]
    pub interjections: u32,

    /// One of them turned down what was being done.
    #[serde(default)]
    pub rejected_mid_run: bool,

    /// The run was carried on after a provider failure.
    #[serde(default)]
    pub resumed: bool,

    /// How the person took the run, from what they said next in the same
    /// session. Not in the run's own line: it is joined on from a later
    /// one when the ledger is read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reaction: Option<Reaction>,
}

/// A line of the ledger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "line", rename_all = "snake_case")]
enum Line {
    Run(Box<RunRecord>),

    /// How the person took an earlier run.
    Reaction {
        ts_ms: u64,
        run: String,
        reaction: Reaction,
    },
}

/// The folder of monthly files the runs are recorded in.
#[derive(Debug, Clone)]
pub struct EffectLedger {
    months: Months,
}

/// Which runs to look at.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Filter {
    /// Only runs in this project or place.
    pub place: Option<String>,

    /// Only runs of this model.
    pub model: Option<String>,
}

impl EffectLedger {
    pub fn open(folder: impl Into<PathBuf>) -> Self {
        Self {
            months: Months::new(folder.into()),
        }
    }

    /// The ledger every session of this person writes to, in
    /// `~/.cortex/effect`.
    pub fn at_home() -> Option<Self> {
        Some(Self::open(
            std::env::home_dir()?.join(".cortex").join("effect"),
        ))
    }

    pub fn folder(&self) -> &Path {
        self.months.folder()
    }

    /// Adds a run.
    pub fn append(&self, record: &RunRecord) -> std::io::Result<()> {
        let line = Line::Run(Box::new(record.clone()));

        self.months
            .append(record.ts_ms, &serde_json::to_string(&line)?)
    }

    /// Adds how the person took the run named `run`.
    pub fn append_reaction(
        &self,
        ts_ms: u64,
        run: &str,
        reaction: Reaction,
    ) -> std::io::Result<()> {
        let line = Line::Reaction {
            ts_ms,
            run: run.to_string(),
            reaction,
        };

        self.months.append(ts_ms, &serde_json::to_string(&line)?)
    }

    /// The runs that started from `from_ms` up to, not including,
    /// `to_ms`, oldest first, each with the person's reaction when one
    /// was recorded.
    pub fn runs(&self, from_ms: u64, to_ms: u64) -> Vec<RunRecord> {
        // A reaction is written when the person next speaks, which can
        // be in the month after the run.
        let lines: Vec<Line> = self
            .months
            .spanning(from_ms, to_ms.saturating_add(32 * DAY_MS as u64));

        let mut reactions: BTreeMap<String, Reaction> = BTreeMap::new();
        let mut runs = Vec::new();

        for line in lines {
            match line {
                Line::Run(record) if (from_ms..to_ms).contains(&record.ts_ms) => runs.push(*record),
                Line::Run(_) => {}

                Line::Reaction { run, reaction, .. } => {
                    reactions.insert(run, reaction);
                }
            }
        }

        for run in &mut runs {
            run.reaction = reactions.get(&run.run).copied();
        }

        runs.sort_by_key(|run| run.ts_ms);

        runs
    }

    /// The comparison of every adviser over a span of time.
    pub fn report(
        &self,
        from_ms: u64,
        to_ms: u64,
        filter: &Filter,
        utc_offset_minutes: i32,
    ) -> EffectReport {
        let mut runs = self.runs(from_ms, to_ms);

        runs.retain(|run| {
            filter
                .place
                .as_ref()
                .is_none_or(|place| run.place == *place)
                && filter
                    .model
                    .as_ref()
                    .is_none_or(|model| run.model == *model)
        });

        let mut report = analyse(&runs, utc_offset_minutes);

        report.from_ms = from_ms;
        report.to_ms = to_ms;

        report
    }
}

/// What a run is measured by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Metric {
    /// Share of runs whose checks passed, among runs whose checks ran.
    ChecksPassed,

    /// Share of runs the person accepted, among runs the person reacted
    /// to, by the first-word test of [`reaction`]. A run turned down
    /// while it was going counts as rejected.
    Accepted,

    /// Model calls, among runs that finished.
    Steps,

    /// Tool calls that failed, among runs that finished.
    FailedToolCalls,

    /// Tokens in and out, among runs that finished.
    Tokens,

    /// Milliseconds, among runs that finished.
    DurationMs,
}

impl Metric {
    pub const ALL: [Self; 6] = [
        Self::ChecksPassed,
        Self::Accepted,
        Self::Steps,
        Self::FailedToolCalls,
        Self::Tokens,
        Self::DurationMs,
    ];

    /// Whether the metric is a share of runs, not an amount per run.
    pub fn is_rate(self) -> bool {
        matches!(self, Self::ChecksPassed | Self::Accepted)
    }

    /// Whether more of it is better.
    pub fn higher_is_better(self) -> bool {
        self.is_rate()
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::ChecksPassed => "checks passed",
            Self::Accepted => "accepted",
            Self::Steps => "steps",
            Self::FailedToolCalls => "failed tool calls",
            Self::Tokens => "tokens",
            Self::DurationMs => "duration (ms)",
        }
    }

    /// The run's value for this metric, or `None` when the run does not
    /// count towards it.
    pub fn of(self, run: &RunRecord) -> Option<f64> {
        let finished = run.ending == Ending::Finished;

        match self {
            Self::ChecksPassed => run.verified.map(|passed| f64::from(u8::from(passed))),

            Self::Accepted => match (run.rejected_mid_run, run.reaction) {
                (true, _) | (_, Some(Reaction::Rejected)) => Some(0.0),
                (false, Some(Reaction::Accepted)) => Some(1.0),
                (false, None) => None,
            },

            Self::Steps => finished.then_some(f64::from(run.steps)),
            Self::FailedToolCalls => finished.then_some(f64::from(run.failed_tool_calls)),
            Self::Tokens => finished.then_some(run.usage.total() as f64),
            Self::DurationMs => finished.then_some(run.duration_ms as f64),
        }
    }
}

/// What a comparison shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Runs went better with the adviser speaking, by more than chance
    /// explains at the 95% level.
    Helps,

    /// Runs went worse with it speaking, likewise.
    Hurts,

    /// There are enough runs and the interval includes no difference.
    /// That is not proof there is none: a small effect needs more runs.
    NoDifferenceSeen,

    /// Fewer than [`MIN_PER_ARM`] runs on one side or the other.
    TooFewRuns,
}

/// One side of a comparison.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ArmStat {
    /// Runs that count towards the metric.
    pub n: u64,

    /// Their mean; for a rate, the share. `None` without runs.
    pub mean: Option<f64>,
}

/// One metric compared between the runs an adviser spoke in and the runs
/// it was held out of.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricEffect {
    pub metric: Metric,
    pub higher_is_better: bool,
    pub spoke: ArmStat,
    pub held_out: ArmStat,

    /// The mean with the adviser speaking less the mean with it held
    /// out.
    pub difference: Option<f64>,

    /// The 95% interval of the difference: Newcombe's from Wilson
    /// intervals for a rate, Welch's for a mean.
    pub interval: Option<[f64; 2]>,

    pub verdict: Verdict,
}

/// A metric among the runs in which the model mostly did what the
/// adviser said and those in which it mostly did not. Not a controlled
/// comparison: see the module's cautions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FollowedObservational {
    pub metric: Metric,
    pub followed: ArmStat,
    pub not_followed: ArmStat,
}

/// One week of one metric, for a chart.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WeekPoint {
    /// The Monday the week begins on, `YYYY-MM-DD`, local.
    pub week: String,

    pub metric: Metric,
    pub spoke: ArmStat,
    pub held_out: ArmStat,
}

/// Everything known about one adviser, or one source of briefings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceEffect {
    pub source: String,

    /// Runs in which it spoke, runs it was held out of in which it would
    /// have spoken, and runs in which it had nothing to say. The
    /// comparison is between the first two.
    pub runs_spoke: u64,
    pub runs_held_out: u64,
    pub runs_nothing_to_say: u64,

    /// Runs it was held out of in which it would have had nothing to say
    /// either. They are in neither side of the comparison.
    pub runs_held_out_silent: u64,

    /// One for every metric in [`Metric::ALL`], in that order.
    pub metrics: Vec<MetricEffect>,

    /// Every week with a run, oldest first, each metric in turn.
    pub weekly: Vec<WeekPoint>,

    /// Not causal. Times it advised in runs in which it spoke.
    pub times_advised_observational: u64,

    /// Not causal. Of those, the times the advice named something that
    /// could be checked, and the times the model's next response did it.
    pub comparable_observational: u64,
    pub followed_observational: u64,

    /// Not causal. How runs went when the model mostly followed the
    /// advice and when it mostly did not.
    pub outcome_when_followed_observational: Vec<FollowedObservational>,
}

/// The comparison of every adviser.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectReport {
    pub from_ms: u64,
    pub to_ms: u64,

    /// Runs looked at, and how many of them were part of the experiment.
    pub runs: u64,
    pub runs_in_experiment: u64,

    /// Runs needed on each side before a verdict is given.
    pub min_per_arm: u64,

    /// How many comparisons with enough runs for a verdict were made. At
    /// a 95% interval about one in twenty of them shows a difference by
    /// chance alone.
    pub comparisons: u32,

    /// Every run in which some adviser had something to say: those in
    /// which all of them spoke against those in which at least one was
    /// held out.
    pub overall: SourceEffect,

    /// By name.
    pub advisers: Vec<SourceEffect>,

    /// The sources of briefings, when briefings are held out too.
    pub briefings: Vec<SourceEffect>,
}

/// Which side of a comparison a run is on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Spoke,
    HeldOut,
}

/// Compares the runs each adviser spoke in with the runs it was held out
/// of. Weeks are the person's own: `utc_offset_minutes` is the offset of
/// local time from UTC.
pub fn analyse(runs: &[RunRecord], utc_offset_minutes: i32) -> EffectReport {
    let mut advisers: BTreeMap<&str, Vec<(&RunRecord, &SourceRun)>> = BTreeMap::new();
    let mut briefings: BTreeMap<&str, Vec<(&RunRecord, &SourceRun)>> = BTreeMap::new();

    for run in runs.iter().filter(|run| run.seed.is_some()) {
        for adviser in &run.advisers {
            advisers
                .entry(&adviser.source)
                .or_default()
                .push((run, adviser));
        }

        for briefing in &run.briefings {
            briefings
                .entry(&briefing.source)
                .or_default()
                .push((run, briefing));
        }
    }

    let effect = |(source, of): (&str, Vec<(&RunRecord, &SourceRun)>)| {
        source_effect(source, &of, utc_offset_minutes)
    };

    let advisers: Vec<SourceEffect> = advisers.into_iter().map(effect).collect();
    let briefings: Vec<SourceEffect> = briefings.into_iter().map(effect).collect();

    // All of them together: stand-ins for one adviser that spoke when
    // every adviser with something to say was shown.
    let together: Vec<(RunRecord, SourceRun)> = runs
        .iter()
        .filter(|run| run.seed.is_some())
        .filter_map(|run| {
            let vocal: Vec<&SourceRun> = run
                .advisers
                .iter()
                .filter(|adviser| adviser.advised > 0)
                .collect();

            (!vocal.is_empty()).then(|| {
                let all = SourceRun {
                    source: "all advisers".to_string(),
                    held_out: vocal.iter().any(|adviser| adviser.held_out),
                    advised: vocal.iter().map(|adviser| adviser.advised).sum(),
                    comparable: vocal.iter().map(|adviser| adviser.comparable).sum(),
                    followed: vocal.iter().map(|adviser| adviser.followed).sum(),
                };

                (run.clone(), all)
            })
        })
        .collect();

    let together: Vec<(&RunRecord, &SourceRun)> =
        together.iter().map(|(run, all)| (run, all)).collect();

    let overall = source_effect("all advisers", &together, utc_offset_minutes);

    let comparisons = advisers
        .iter()
        .chain(&briefings)
        .chain([&overall])
        .flat_map(|source| &source.metrics)
        .filter(|metric| metric.verdict != Verdict::TooFewRuns)
        .count() as u32;

    EffectReport {
        from_ms: runs.first().map(|run| run.ts_ms).unwrap_or_default(),
        to_ms: runs.last().map(|run| run.ts_ms + 1).unwrap_or_default(),
        runs: runs.len() as u64,
        runs_in_experiment: runs.iter().filter(|run| run.seed.is_some()).count() as u64,
        min_per_arm: MIN_PER_ARM,
        comparisons,
        overall,
        advisers,
        briefings,
    }
}

fn source_effect(
    source: &str,
    runs: &[(&RunRecord, &SourceRun)],
    utc_offset_minutes: i32,
) -> SourceEffect {
    let count = |held_out: bool, vocal: bool| {
        runs.iter()
            .filter(|(_, of)| of.held_out == held_out && (of.advised > 0) == vocal)
            .count() as u64
    };

    // Only runs in which it had something to say. That is known on both
    // sides, since a held-out adviser is still asked, and the first time
    // it speaks comes before its speaking has changed anything.
    let sided: Vec<(&RunRecord, &SourceRun, Side)> = runs
        .iter()
        .filter(|(_, of)| of.advised > 0)
        .map(|(run, of)| {
            let side = if of.held_out {
                Side::HeldOut
            } else {
                Side::Spoke
            };

            (*run, *of, side)
        })
        .collect();

    let values = |metric: Metric, side: Side| -> Vec<f64> {
        sided
            .iter()
            .filter(|(_, _, of)| *of == side)
            .filter_map(|(run, _, _)| metric.of(run))
            .collect()
    };

    let metrics = Metric::ALL
        .into_iter()
        .map(|metric| {
            compare(
                metric,
                &values(metric, Side::Spoke),
                &values(metric, Side::HeldOut),
            )
        })
        .collect();

    let mut weeks: BTreeMap<i64, Vec<(&RunRecord, Side)>> = BTreeMap::new();

    for (run, _, side) in &sided {
        let day = ledger::local_day(run.ts_ms, utc_offset_minutes);
        let monday = day - i64::from(ledger::weekday(day));

        weeks.entry(monday).or_default().push((run, *side));
    }

    let weekly = weeks
        .into_iter()
        .flat_map(|(monday, of_week)| {
            Metric::ALL.into_iter().map(move |metric| {
                let side = |side: Side| -> Vec<f64> {
                    of_week
                        .iter()
                        .filter(|(_, of)| *of == side)
                        .filter_map(|(run, _)| metric.of(run))
                        .collect()
                };

                WeekPoint {
                    week: ledger::date_text(monday),
                    metric,
                    spoke: arm_stat(&side(Side::Spoke)),
                    held_out: arm_stat(&side(Side::HeldOut)),
                }
            })
        })
        .collect();

    let spoke: Vec<(&RunRecord, &SourceRun)> = sided
        .iter()
        .filter(|(_, _, side)| *side == Side::Spoke)
        .map(|(run, of, _)| (*run, *of))
        .collect();

    let sum = |field: fn(&SourceRun) -> u32| -> u64 {
        spoke.iter().map(|(_, of)| u64::from(field(of))).sum()
    };

    let outcome_when_followed_observational = Metric::ALL
        .into_iter()
        .map(|metric| {
            let split = |followed: bool| -> Vec<f64> {
                spoke
                    .iter()
                    .filter(|(_, of)| of.comparable > 0)
                    .filter(|(_, of)| (of.followed * 2 >= of.comparable) == followed)
                    .filter_map(|(run, _)| metric.of(run))
                    .collect()
            };

            FollowedObservational {
                metric,
                followed: arm_stat(&split(true)),
                not_followed: arm_stat(&split(false)),
            }
        })
        .collect();

    SourceEffect {
        source: source.to_string(),
        runs_spoke: count(false, true),
        runs_held_out: count(true, true),
        runs_nothing_to_say: count(false, false),
        runs_held_out_silent: count(true, false),
        metrics,
        weekly,
        times_advised_observational: sum(|of| of.advised),
        comparable_observational: sum(|of| of.comparable),
        followed_observational: sum(|of| of.followed),
        outcome_when_followed_observational,
    }
}

fn arm_stat(values: &[f64]) -> ArmStat {
    ArmStat {
        n: values.len() as u64,
        mean: mean(values),
    }
}

fn mean(values: &[f64]) -> Option<f64> {
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

/// Compares one metric between the two sides.
pub fn compare(metric: Metric, spoke: &[f64], held_out: &[f64]) -> MetricEffect {
    let (with, without) = (arm_stat(spoke), arm_stat(held_out));

    let difference = with
        .mean
        .zip(without.mean)
        .map(|(with, without)| with - without);

    let interval = if spoke.is_empty() || held_out.is_empty() {
        None
    } else if metric.is_rate() {
        Some(newcombe(spoke, held_out))
    } else {
        welch(spoke, held_out)
    };

    let enough = with.n >= MIN_PER_ARM && without.n >= MIN_PER_ARM;

    let verdict = match interval {
        _ if !enough => Verdict::TooFewRuns,
        None => Verdict::TooFewRuns,

        Some([low, high]) => {
            let better = if metric.higher_is_better() {
                low > 0.0
            } else {
                high < 0.0
            };

            let worse = if metric.higher_is_better() {
                high < 0.0
            } else {
                low > 0.0
            };

            match (better, worse) {
                (true, _) => Verdict::Helps,
                (_, true) => Verdict::Hurts,
                _ => Verdict::NoDifferenceSeen,
            }
        }
    };

    MetricEffect {
        metric,
        higher_is_better: metric.higher_is_better(),
        spoke: with,
        held_out: without,
        difference,
        interval,
        verdict,
    }
}

/// The normal deviate that leaves 2.5% in each tail.
const Z: f64 = 1.959_963_984_540_054;

/// Wilson's 95% interval for a share of `n` trials.
fn wilson(successes: f64, n: f64) -> (f64, f64) {
    let share = successes / n;
    let centre = share + Z * Z / (2.0 * n);
    let spread = Z * (share * (1.0 - share) / n + Z * Z / (4.0 * n * n)).sqrt();
    let scale = 1.0 + Z * Z / n;

    ((centre - spread) / scale, (centre + spread) / scale)
}

/// Newcombe's 95% interval for the difference of two shares, built from
/// the Wilson interval of each. The values are 0 or 1.
fn newcombe(first: &[f64], second: &[f64]) -> [f64; 2] {
    let (n1, n2) = (first.len() as f64, second.len() as f64);
    let (p1, p2) = (
        first.iter().sum::<f64>() / n1,
        second.iter().sum::<f64>() / n2,
    );

    let (low1, high1) = wilson(p1 * n1, n1);
    let (low2, high2) = wilson(p2 * n2, n2);

    let difference = p1 - p2;

    [
        difference - ((p1 - low1).powi(2) + (high2 - p2).powi(2)).sqrt(),
        difference + ((high1 - p1).powi(2) + (p2 - low2).powi(2)).sqrt(),
    ]
}

/// Welch's 95% interval for the difference of two means, which does not
/// assume the two sides vary alike. `None` when a side has fewer than
/// two values, so that its spread cannot be told.
fn welch(first: &[f64], second: &[f64]) -> Option<[f64; 2]> {
    if first.len() < 2 || second.len() < 2 {
        return None;
    }

    let (n1, n2) = (first.len() as f64, second.len() as f64);
    let (mean1, mean2) = (mean(first)?, mean(second)?);

    let variance = |values: &[f64], mean: f64, n: f64| {
        values
            .iter()
            .map(|value| (value - mean).powi(2))
            .sum::<f64>()
            / (n - 1.0)
    };

    let (part1, part2) = (
        variance(first, mean1, n1) / n1,
        variance(second, mean2, n2) / n2,
    );

    let difference = mean1 - mean2;
    let error = (part1 + part2).sqrt();

    if error == 0.0 {
        return Some([difference, difference]);
    }

    // Welch and Satterthwaite's degrees of freedom.
    let freedom =
        (part1 + part2).powi(2) / (part1.powi(2) / (n1 - 1.0) + part2.powi(2) / (n2 - 1.0));

    let reach = student_975(freedom) * error;

    Some([difference - reach, difference + reach])
}

/// The value Student's t leaves 2.5% above, for `freedom` degrees of
/// freedom, by the usual expansion around the normal deviate. Within a
/// hundredth of the exact value from three degrees of freedom up.
fn student_975(freedom: f64) -> f64 {
    let z = Z;
    let v = freedom.max(1.0);

    z + (z.powi(3) + z) / (4.0 * v)
        + (5.0 * z.powi(5) + 16.0 * z.powi(3) + 3.0 * z) / (96.0 * v * v)
        + (3.0 * z.powi(7) + 19.0 * z.powi(5) + 17.0 * z.powi(3) - 15.0 * z) / (384.0 * v.powi(3))
}

/// Writes every run it hears about to an [`EffectLedger`].
///
/// A run is written when it reports its result. A run that never does
/// is written when the next one starts, when the person stops it
/// ([`Self::stopped_by_person`]), or when the sink is dropped. How the
/// person took a run is written as a line of its own when they next
/// speak in the same session.
pub struct EffectSink {
    ledger: EffectLedger,
    place: String,
    stand_ins: bool,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    session: String,

    /// The run in progress.
    run: Option<RunRecord>,

    /// The model has answered at least once in it.
    answered: bool,

    /// Advice given since the model last answered that names what it
    /// should do next: whose it was and what.
    expected: Vec<(String, Expected)>,

    /// The last run that ended, until the person has spoken again.
    awaiting_reaction: Option<String>,

    runs: u64,
}

impl EffectSink {
    /// A sink for sessions that work in `place`.
    pub fn new(ledger: EffectLedger, place: &str) -> Self {
        Self {
            ledger,
            place: place.to_string(),
            stand_ins: false,
            state: Mutex::new(State {
                session: format!("{}-{}", unix_millis(), std::process::id()),
                ..State::default()
            }),
        }
    }

    /// Also records the runs of the stand-in models that tests and the
    /// demonstration use.
    pub fn recording_stand_ins(mut self) -> Self {
        self.stand_ins = true;

        self
    }

    /// Files what follows under `session`, as
    /// [`crate::ledger::LedgerSink::set_session`] does. What the person
    /// says first in another session is not a reaction to the last run
    /// of this one.
    pub fn set_session(&self, session: &str) {
        let mut state = self.lock();

        if state.session != session {
            state.session = session.to_string();
            state.awaiting_reaction = None;
        }
    }

    /// The person stopped the run in progress. Call it when a run's
    /// future is dropped because they asked.
    pub fn stopped_by_person(&self) {
        let mut state = self.lock();

        self.close(&mut state, Ending::Stopped);
    }

    pub fn ledger(&self) -> &EffectLedger {
        &self.ledger
    }

    /// The hold-outs of the run in progress: the advisers and briefings
    /// known so far to be kept from the model in it.
    pub fn held_out_now(&self) -> Vec<String> {
        let state = self.lock();

        state
            .run
            .iter()
            .flat_map(|run| run.advisers.iter().chain(&run.briefings))
            .filter(|source| source.held_out)
            .map(|source| source.source.clone())
            .collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Writes the run in progress, if there is one, as having ended this
    /// way.
    fn close(&self, state: &mut State, ending: Ending) {
        let Some(mut run) = state.run.take() else {
            return;
        };

        state.expected.clear();

        run.ending = ending;

        if !self.stand_ins && is_stand_in(&run.provider) {
            return;
        }

        if ending != Ending::Finished {
            run.duration_ms = (unix_millis() as u64).saturating_sub(run.ts_ms);
        }

        match self.ledger.append(&run) {
            Ok(()) => state.awaiting_reaction = Some(run.run),

            Err(error) => eprintln!(
                "warning: could not write to the effect ledger in {}: {error}",
                self.ledger.folder().display()
            ),
        }
    }

    /// What the person has just said, as a reaction to the last run.
    fn hear(&self, state: &mut State, text: &str) {
        let Some(run) = state.awaiting_reaction.take() else {
            return;
        };

        let Some(reaction) = reaction(text) else {
            return;
        };

        if let Err(error) = self
            .ledger
            .append_reaction(unix_millis() as u64, &run, reaction)
        {
            eprintln!("warning: could not write to the effect ledger: {error}");
        }
    }
}

/// The entry for `source` in `sources`, made if it is not there.
fn entry<'a>(sources: &'a mut Vec<SourceRun>, source: &str) -> &'a mut SourceRun {
    let at = match sources.iter().position(|known| known.source == source) {
        Some(at) => at,

        None => {
            sources.push(SourceRun {
                source: source.to_string(),
                ..SourceRun::default()
            });

            sources.len() - 1
        }
    };

    &mut sources[at]
}

impl TrajectorySink for EffectSink {
    fn record(&self, event: &Event) {
        let mut state = self.lock();

        match event {
            Event::RunStarted {
                instruction,
                provider,
                model,
            } => {
                // The run before this one never reported.
                self.close(&mut state, Ending::Unfinished);
                self.hear(&mut state, instruction);

                let ts_ms = unix_millis() as u64;
                let chars = instruction.chars().count() as u64;

                state.runs += 1;
                state.answered = false;

                state.run = Some(RunRecord {
                    ts_ms,
                    run: format!("{}:{ts_ms}:{}", state.session, state.runs),
                    session: state.session.clone(),
                    place: self.place.clone(),
                    provider: provider.clone(),
                    model: model.clone(),
                    instruction_chars: chars,
                    task_size: TaskSize::of(chars),
                    ..RunRecord::default()
                });
            }

            Event::RunFinished {
                outcome,
                steps,
                usage,
                duration_ms,
                verified,
            } => {
                if let Some(run) = &mut state.run {
                    run.outcome = Some(*outcome);
                    run.steps = *steps;
                    run.usage = *usage;
                    run.duration_ms = *duration_ms;
                    run.verified = *verified;
                }

                self.close(&mut state, Ending::Finished);
            }

            _ => {
                let State {
                    run: Some(run),
                    answered,
                    expected,
                    ..
                } = &mut *state
                else {
                    return;
                };

                match event {
                    Event::ExperimentAssigned {
                        seed,
                        share,
                        held_out,
                        speaking,
                        briefed,
                        ..
                    } => {
                        run.seed = Some(*seed);
                        run.share = *share;

                        for source in held_out {
                            entry(&mut run.advisers, source).held_out = true;
                        }

                        for source in speaking {
                            entry(&mut run.advisers, source);
                        }

                        for source in briefed {
                            entry(&mut run.briefings, source).advised = 1;
                        }
                    }

                    // Before the model has answered it is a briefing.
                    // Those are listed when the hold-outs are.
                    Event::ContextPrepared { source, detail, .. } if *answered => {
                        entry(&mut run.advisers, source).advised += 1;

                        expected.extend(expectation(detail).map(|what| (source.clone(), what)));
                    }

                    Event::ContextWithheld {
                        step: 0, source, ..
                    } => {
                        let briefing = entry(&mut run.briefings, source);

                        briefing.held_out = true;
                        briefing.advised = 1;
                    }

                    Event::ContextWithheld { source, detail, .. } => {
                        let adviser = entry(&mut run.advisers, source);

                        adviser.held_out = true;
                        adviser.advised += 1;

                        expected.extend(expectation(detail).map(|what| (source.clone(), what)));
                    }

                    Event::ModelResponded {
                        message,
                        stop_reason,
                        usage,
                        ..
                    } => {
                        *answered = true;

                        run.steps += 1;
                        run.usage += *usage;

                        for (source, what) in expected.drain(..) {
                            let adviser = entry(&mut run.advisers, &source);

                            adviser.comparable += 1;
                            adviser.followed += u32::from(met(&what, message, stop_reason));
                        }
                    }

                    Event::ToolStarted { .. } => run.tool_calls += 1,

                    Event::ToolFinished { result, .. } if result.is_error => {
                        run.failed_tool_calls += 1;
                    }

                    Event::VerificationRan { passed, .. } => {
                        run.verification_rounds += 1;
                        run.verified = Some(*passed);
                    }

                    Event::UserInterjected { text, .. } => {
                        run.interjections += 1;

                        if reaction(text) == Some(Reaction::Rejected) {
                            run.rejected_mid_run = true;
                        }
                    }

                    Event::RunResumed { .. } => run.resumed = true,

                    _ => {}
                }
            }
        }
    }
}

impl Drop for EffectSink {
    fn drop(&mut self) {
        let mut state = self.lock();

        self.close(&mut state, Ending::Unfinished);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tempfile::TempDir;

    use super::*;
    use crate::message::{Block, Role, ToolCall, ToolResult};

    #[test]
    fn the_same_seed_and_name_give_the_same_answer() {
        for seed in [0, 1, 42, u64::MAX] {
            for source in ["intuition", "mind", "dendritic network"] {
                assert_eq!(
                    held_out(seed, source, 0.25),
                    held_out(seed, source, 0.25),
                    "{seed} {source}"
                );
            }
        }

        assert!(!held_out(7, "intuition", 0.0));
        assert!(held_out(7, "intuition", 1.0));
    }

    #[test]
    fn about_the_set_share_of_runs_is_held_out_and_advisers_are_independent() {
        let runs = 20_000u64;

        let count = |source: &str| {
            (0..runs)
                .filter(|seed| held_out(*seed, source, 0.25))
                .count()
        };

        for source in [
            "intuition",
            "mind",
            "dendritic network",
            "baseline controller",
        ] {
            let share = count(source) as f64 / runs as f64;

            assert!((share - 0.25).abs() < 0.02, "{source}: {share}");
        }

        // Held out together about as often as two coins both landing
        // that way: one in sixteen.
        let both = (0..runs)
            .filter(|seed| held_out(*seed, "intuition", 0.25) && held_out(*seed, "mind", 0.25))
            .count() as f64
            / runs as f64;

        assert!((both - 0.0625).abs() < 0.01, "{both}");
    }

    #[test]
    fn a_pinned_seed_is_used_and_nothing_to_hold_out_means_no_experiment() {
        let pinned = ExperimentConfig {
            seed: Some(99),
            ..ExperimentConfig::default()
        };

        assert_eq!(Experiment::draw(&pinned).unwrap().seed, 99);

        let off = ExperimentConfig {
            share: 0.0,
            ..ExperimentConfig::default()
        };

        assert_eq!(Experiment::draw(&off), None);

        let first = Experiment::draw(&ExperimentConfig::default()).unwrap();
        let second = Experiment::draw(&ExperimentConfig::default()).unwrap();

        assert_ne!(first.seed, second.seed);
    }

    #[test]
    fn a_skill_is_never_held_out() {
        let all = Experiment {
            seed: 3,
            share: 1.0,
            briefing_share: 1.0,
        };

        assert!(all.withholds_advice("mind"));
        assert!(all.withholds_briefing("memory"));
        assert!(!all.withholds_briefing("skill"));
        assert!(!all.withholds_briefing("rollout"));

        let advisers_only = Experiment {
            briefing_share: 0.0,
            ..all
        };

        assert!(!advisers_only.withholds_briefing("memory"));
    }

    #[test]
    fn the_first_word_says_how_the_person_took_it() {
        assert_eq!(reaction("no, revert that"), Some(Reaction::Rejected));
        assert_eq!(reaction("Perfect, thanks"), Some(Reaction::Accepted));
        assert_eq!(reaction("hm, that's wrong"), Some(Reaction::Rejected));
        assert_eq!(reaction("add a currency field"), None);
    }

    fn response(calls: &[(&str, Value)]) -> (Message, StopReason) {
        let blocks: Vec<Block> = calls
            .iter()
            .enumerate()
            .map(|(index, (name, input))| {
                Block::ToolCall(ToolCall {
                    id: format!("call_{index}"),
                    name: name.to_string(),
                    input: input.clone(),
                })
            })
            .collect();

        let stop_reason = if blocks.is_empty() {
            StopReason::EndTurn
        } else {
            StopReason::ToolUse
        };

        (
            Message {
                role: Role::Assistant,
                blocks,
            },
            stop_reason,
        )
    }

    #[test]
    fn what_advice_expects_is_read_from_each_advisers_own_record() {
        let ran_tests = response(&[("shell", json!({ "command": "npm  test -- money" }))]);
        let read = response(&[("read_file", json!({ "path": "src/money.ts" }))]);
        let done = response(&[]);

        let follows = |detail: Value, (message, stop_reason): &(Message, StopReason)| {
            expectation(&detail).map(|expected| met(&expected, message, stop_reason))
        };

        // A decision controller.
        assert_eq!(follows(json!({ "advised": "run" }), &ran_tests), Some(true));
        assert_eq!(follows(json!({ "advised": "run" }), &read), Some(false));
        assert_eq!(follows(json!({ "advised": "finish" }), &done), Some(true));

        // The dendritic network.
        let next = |label: &str| json!({ "kind": "next", "label": label });

        assert_eq!(follows(next("does: read_file"), &read), Some(true));
        assert_eq!(follows(next("runs: npm test"), &ran_tests), Some(true));
        assert_eq!(follows(next("runs: cargo test"), &ran_tests), Some(false));
        assert_eq!(follows(next("file: src/money.ts"), &read), Some(true));
        assert_eq!(follows(next("result: failed"), &read), None);

        // A warning is followed by not doing it.
        let warning = json!({ "kind": "warning", "label": "runs: npm test" });

        assert_eq!(follows(warning.clone(), &ran_tests), Some(false));
        assert_eq!(follows(warning, &read), Some(true));

        // The intuition.
        let hunches = json!({ "hunches": [
            { "kind": "command", "text": "npm test" },
            { "kind": "file", "text": "src/money.ts" },
        ]});

        assert_eq!(follows(hunches.clone(), &read), Some(true));
        assert_eq!(follows(hunches, &ran_tests), Some(false));

        // The mind's advice names nothing that can be checked.
        assert_eq!(
            follows(json!({ "advice": "careful", "kind": "habit" }), &read),
            None
        );
    }

    fn ledger() -> (TempDir, EffectLedger) {
        let folder = tempfile::tempdir().unwrap();
        let ledger = EffectLedger::open(folder.path().join("effect"));

        (folder, ledger)
    }

    fn sink() -> (TempDir, EffectSink) {
        let (folder, ledger) = ledger();

        (folder, EffectSink::new(ledger, "/work/project"))
    }

    fn started(instruction: &str) -> Event {
        Event::RunStarted {
            instruction: instruction.to_string(),
            provider: "anthropic".to_string(),
            model: "claude-opus-5-5".to_string(),
        }
    }

    fn responded(step: u32, (message, stop_reason): (Message, StopReason)) -> Event {
        Event::ModelResponded {
            step,
            message,
            stop_reason,
            usage: Usage {
                input_tokens: 100,
                output_tokens: 20,
                ..Usage::default()
            },
            duration_ms: 1,
        }
    }

    fn finished(steps: u32, verified: Option<bool>) -> Event {
        Event::RunFinished {
            outcome: Outcome::Completed,
            steps,
            usage: Usage {
                input_tokens: 500,
                output_tokens: 50,
                ..Usage::default()
            },
            duration_ms: 4_000,
            verified,
        }
    }

    fn advice(source: &str, detail: Value) -> Event {
        Event::ContextPrepared {
            source: source.to_string(),
            text: "a hint".to_string(),
            detail,
            duration_ms: 1,
        }
    }

    fn withheld(step: u32, source: &str, detail: Value) -> Event {
        Event::ContextWithheld {
            step,
            source: source.to_string(),
            text: "a hint".to_string(),
            detail,
            duration_ms: 1,
        }
    }

    fn tool_finished(is_error: bool) -> Event {
        Event::ToolFinished {
            step: 1,
            tool: "read_file".to_string(),
            result: ToolResult {
                call_id: "call_0".to_string(),
                output: "x".to_string(),
                is_error,
                images: Vec::new(),
            },
            duration_ms: 1,
        }
    }

    fn all(sink: &EffectSink) -> Vec<RunRecord> {
        sink.ledger().runs(0, u64::MAX)
    }

    #[test]
    fn a_run_is_recorded_with_what_each_adviser_did_and_how_it_went() {
        let (_folder, sink) = sink();

        sink.set_session("1760000000-42-0");

        sink.record(&started("Fix the rounding"));

        // A briefing, before the model has answered.
        sink.record(&advice("memory", json!({})));

        sink.record(&Event::ExperimentAssigned {
            seed: 7,
            share: 0.25,
            briefing_share: 0.0,
            held_out: vec!["intuition".to_string()],
            speaking: vec!["mind".to_string(), "baseline controller".to_string()],
            briefed: vec!["memory".to_string()],
        });

        let read = || response(&[("read_file", json!({ "path": "src/money.ts" }))]);

        sink.record(&responded(1, read()));
        sink.record(&Event::ToolStarted {
            step: 1,
            call: read().0.tool_calls().remove(0),
        });
        sink.record(&tool_finished(true));

        // The controller says to run something; the intuition would have
        // pointed at the file.
        sink.record(&advice("baseline controller", json!({ "advised": "run" })));
        sink.record(&withheld(
            1,
            "intuition",
            json!({ "hunches": [{ "kind": "file", "text": "src/money.ts" }] }),
        ));

        sink.record(&responded(2, read()));
        sink.record(&tool_finished(false));
        sink.record(&responded(3, response(&[])));

        sink.record(&Event::VerificationRan {
            step: 3,
            passed: true,
            checks: Vec::new(),
            duration_ms: 1,
        });

        sink.record(&finished(3, Some(true)));

        let runs = all(&sink);

        assert_eq!(runs.len(), 1);

        let run = &runs[0];

        assert_eq!(run.session, "1760000000-42-0");
        assert_eq!(run.place, "/work/project");
        assert_eq!(run.seed, Some(7));
        assert_eq!(run.task_size, TaskSize::Short);
        assert_eq!(run.ending, Ending::Finished);
        assert_eq!(run.outcome, Some(Outcome::Completed));
        assert_eq!(run.verified, Some(true));
        assert_eq!(
            (run.steps, run.tool_calls, run.failed_tool_calls),
            (3, 1, 1)
        );
        assert_eq!(run.verification_rounds, 1);
        assert_eq!(run.usage.input_tokens, 500);
        assert_eq!(run.duration_ms, 4_000);

        let adviser = |source: &str| {
            run.advisers
                .iter()
                .find(|adviser| adviser.source == source)
                .unwrap()
        };

        let intuition = adviser("intuition");

        assert_eq!(intuition.arm(), Arm::HeldOut);
        assert_eq!(
            (intuition.advised, intuition.comparable, intuition.followed),
            (1, 1, 1)
        );

        let controller = adviser("baseline controller");

        assert_eq!(controller.arm(), Arm::Spoke);
        assert_eq!(
            (
                controller.advised,
                controller.comparable,
                controller.followed
            ),
            (1, 1, 0)
        );

        assert_eq!(adviser("mind").arm(), Arm::NothingToSay);

        assert_eq!(run.briefings.len(), 1);
        assert_eq!(run.briefings[0].arm(), Arm::Spoke);
    }

    #[test]
    fn what_the_person_says_next_is_joined_to_the_run() {
        let (_folder, sink) = sink();

        sink.record(&started("fix"));
        sink.record(&responded(1, response(&[])));
        sink.record(&finished(1, None));

        sink.record(&started("no, that broke the build"));
        sink.record(&responded(1, response(&[])));
        sink.record(&finished(1, None));

        sink.record(&started("thanks"));
        sink.record(&finished(0, None));

        // Another session: its first words are not about the last run.
        sink.set_session("another");
        sink.record(&started("no"));

        let runs = all(&sink);

        assert_eq!(runs[0].reaction, Some(Reaction::Rejected));
        assert_eq!(runs[1].reaction, Some(Reaction::Accepted));
        assert_eq!(runs[2].reaction, None);
    }

    #[test]
    fn a_run_that_never_reported_is_still_recorded() {
        let (_folder, sink) = sink();

        sink.record(&started("fix"));
        sink.record(&responded(1, response(&[])));
        sink.record(&Event::UserInterjected {
            step: 1,
            text: "stop, wrong file".to_string(),
        });

        sink.stopped_by_person();

        // The provider failed and the run was not carried on.
        sink.record(&started("again"));
        sink.record(&responded(1, response(&[])));
        sink.record(&started("and again"));
        sink.record(&Event::RunResumed { steps: 0 });

        let ledger = sink.ledger().clone();

        // Closing the app.
        drop(sink);

        let runs = ledger.runs(0, u64::MAX);

        let endings: Vec<Ending> = runs.iter().map(|run| run.ending).collect();

        assert_eq!(
            endings,
            [Ending::Stopped, Ending::Unfinished, Ending::Unfinished]
        );

        assert_eq!(runs[0].steps, 1);
        assert_eq!(runs[0].usage.input_tokens, 100);
        assert!(runs[0].rejected_mid_run);
        assert_eq!(runs[0].interjections, 1);
        assert!(runs[2].resumed);

        // Steps and tokens of a run cut short are not compared.
        assert_eq!(Metric::Steps.of(&runs[0]), None);
        assert_eq!(Metric::Accepted.of(&runs[0]), Some(0.0));
    }

    #[test]
    fn a_torn_line_is_skipped_and_does_not_spoil_the_next() {
        let (_folder, sink) = sink();

        sink.record(&started("fix"));
        sink.record(&finished(1, None));

        let file = std::fs::read_dir(sink.ledger().folder())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();

        let mut text = std::fs::read_to_string(&file).unwrap();

        text.push_str("{\"line\":\"run\",\"ts_ms\":17");

        std::fs::write(&file, text).unwrap();

        assert_eq!(all(&sink).len(), 1);

        sink.record(&started("more"));
        sink.record(&finished(1, None));

        assert_eq!(all(&sink).len(), 2);
    }

    #[test]
    fn a_stand_in_models_runs_are_not_recorded() {
        let (_folder, sink) = sink();

        sink.record(&Event::RunStarted {
            instruction: "fix".to_string(),
            provider: "scripted".to_string(),
            model: "scripted".to_string(),
        });
        sink.record(&finished(1, None));

        assert!(all(&sink).is_empty());
    }

    /// A run in the experiment in which `source` had something to say.
    fn run(index: u64, source: &str, held_out: bool, passed: bool, steps: u32) -> RunRecord {
        RunRecord {
            // A run every six hours from 5 October 2026, a Monday.
            ts_ms: 1_791_158_400_000 + index * 6 * 3_600_000,
            run: format!("run-{index}"),
            place: "/work/project".to_string(),
            model: if index.is_multiple_of(2) {
                "big"
            } else {
                "small"
            }
            .to_string(),
            seed: Some(index),
            share: 0.25,
            advisers: vec![SourceRun {
                source: source.to_string(),
                held_out,
                advised: 2,
                comparable: 2,
                followed: if index.is_multiple_of(3) { 2 } else { 0 },
            }],
            verified: Some(passed),
            steps,
            usage: Usage {
                input_tokens: u64::from(steps) * 1_000,
                ..Usage::default()
            },
            duration_ms: u64::from(steps) * 2_000,
            ..RunRecord::default()
        }
    }

    fn metric(effect: &SourceEffect, metric: Metric) -> &MetricEffect {
        effect
            .metrics
            .iter()
            .find(|of| of.metric == metric)
            .unwrap()
    }

    #[test]
    fn a_few_runs_get_no_verdict_however_they_look() {
        let mut runs = Vec::new();

        // Every run with the adviser passed, every run without failed:
        // five of each.
        for index in 0..10 {
            let held_out = index % 2 == 1;

            runs.push(run(index, "intuition", held_out, !held_out, 10));
        }

        let report = analyse(&runs, 0);

        let intuition = &report.advisers[0];

        assert_eq!((intuition.runs_spoke, intuition.runs_held_out), (5, 5));

        let passed = metric(intuition, Metric::ChecksPassed);

        assert_eq!(passed.difference, Some(1.0));
        assert_eq!(passed.verdict, Verdict::TooFewRuns);
        assert_eq!(report.comparisons, 0);
        assert_eq!(report.min_per_arm, 20);
    }

    #[test]
    fn a_planted_difference_is_found_and_its_absence_is_not_called_one() {
        let mut runs = Vec::new();

        for index in 0..200u64 {
            let held_out = index % 2 == 1;

            // With the adviser speaking nine runs in ten pass and a run
            // takes about 10 steps; held out, five in ten and about 16.
            let passed = if held_out {
                index % 4 == 1
            } else {
                index % 20 != 0
            };

            let steps = if held_out {
                14 + index % 5
            } else {
                8 + index % 5
            };

            runs.push(run(index, "helpful", held_out, passed, steps as u32));

            // An adviser that changes nothing: the same pattern of
            // passing and steps on both sides.
            let same = (index / 2) % 3 != 0;

            runs.push(run(
                1_000 + index,
                "idle",
                held_out,
                same,
                (10 + (index / 2) % 4) as u32,
            ));
        }

        let report = analyse(&runs, 0);

        assert_eq!(report.runs, 400);
        assert_eq!(report.runs_in_experiment, 400);

        let helpful = report
            .advisers
            .iter()
            .find(|of| of.source == "helpful")
            .unwrap();

        assert_eq!((helpful.runs_spoke, helpful.runs_held_out), (100, 100));

        let passed = metric(helpful, Metric::ChecksPassed);

        assert_eq!(passed.spoke.mean, Some(0.9));
        assert_eq!(passed.held_out.mean, Some(0.5));
        assert_eq!(passed.verdict, Verdict::Helps);

        let [low, high] = passed.interval.unwrap();

        assert!(low > 0.25 && high < 0.55, "{low} {high}");

        // Fewer steps is better, so a lower mean helps.
        let steps = metric(helpful, Metric::Steps);

        assert!(steps.difference.unwrap() < -5.0);
        assert_eq!(steps.verdict, Verdict::Helps);
        assert_eq!(metric(helpful, Metric::Tokens).verdict, Verdict::Helps);

        // Nobody reacted to any run.
        assert_eq!(
            metric(helpful, Metric::Accepted).verdict,
            Verdict::TooFewRuns
        );

        let idle = report
            .advisers
            .iter()
            .find(|of| of.source == "idle")
            .unwrap();

        assert_eq!(
            metric(idle, Metric::ChecksPassed).verdict,
            Verdict::NoDifferenceSeen
        );
        assert_eq!(
            metric(idle, Metric::Steps).verdict,
            Verdict::NoDifferenceSeen
        );

        // Five metrics with enough runs for each of two advisers and for
        // all of them together.
        assert_eq!(report.comparisons, 15);

        // What the model did after the advice is told apart from that.
        assert_eq!(helpful.times_advised_observational, 200);
        assert_eq!(helpful.comparable_observational, 200);
        assert!(helpful.followed_observational > 0);
        assert_eq!(helpful.outcome_when_followed_observational.len(), 6);
    }

    #[test]
    fn an_adviser_that_hurts_is_said_to() {
        let runs: Vec<RunRecord> = (0..120u64)
            .map(|index| {
                let held_out = index % 2 == 1;

                run(index, "noisy", held_out, held_out || index % 4 == 0, 10)
            })
            .collect();

        let report = analyse(&runs, 0);

        assert_eq!(
            metric(&report.advisers[0], Metric::ChecksPassed).verdict,
            Verdict::Hurts
        );
    }

    #[test]
    fn runs_in_which_it_had_nothing_to_say_are_on_neither_side() {
        let mut silent = run(0, "intuition", false, true, 5);

        silent.advisers[0].advised = 0;

        let mut held_and_silent = run(1, "intuition", true, true, 5);

        held_and_silent.advisers[0].advised = 0;

        // A run from before the experiment, or with it switched off.
        let mut outside = run(2, "intuition", false, true, 5);

        outside.seed = None;

        let report = analyse(&[silent, held_and_silent, outside], 0);

        let intuition = &report.advisers[0];

        assert_eq!((intuition.runs_spoke, intuition.runs_held_out), (0, 0));
        assert_eq!(intuition.runs_nothing_to_say, 1);
        assert_eq!(intuition.runs_held_out_silent, 1);
        assert_eq!(report.runs, 3);
        assert_eq!(report.runs_in_experiment, 2);
    }

    #[test]
    fn each_week_has_a_point_for_each_metric() {
        // Four runs a day for ten days from a Monday: two weeks.
        let runs: Vec<RunRecord> = (0..40u64)
            .map(|index| run(index, "intuition", index % 2 == 1, true, 10))
            .collect();

        let report = analyse(&runs, 0);

        let weekly = &report.advisers[0].weekly;

        assert_eq!(weekly.len(), 2 * Metric::ALL.len());
        assert_eq!(weekly[0].week, "2026-10-05");
        assert_eq!(weekly[6].week, "2026-10-12");

        let steps = weekly
            .iter()
            .find(|point| point.metric == Metric::Steps)
            .unwrap();

        assert_eq!(steps.spoke.n + steps.held_out.n, 28);
        assert_eq!(steps.spoke.mean, Some(10.0));
    }

    #[test]
    fn a_report_can_be_narrowed_to_a_project_or_a_model() {
        let (_folder, ledger) = ledger();

        for index in 0..6 {
            ledger
                .append(&run(index, "intuition", false, true, 5))
                .unwrap();
        }

        let everything = ledger.report(0, u64::MAX, &Filter::default(), 0);

        assert_eq!(everything.runs, 6);
        assert_eq!((everything.from_ms, everything.to_ms), (0, u64::MAX));

        let big = Filter {
            model: Some("big".to_string()),
            ..Filter::default()
        };

        assert_eq!(ledger.report(0, u64::MAX, &big, 0).runs, 3);

        let elsewhere = Filter {
            place: Some("/other".to_string()),
            ..Filter::default()
        };

        assert_eq!(ledger.report(0, u64::MAX, &elsewhere, 0).runs, 0);
    }

    #[test]
    fn the_intervals_agree_with_worked_examples() {
        // Wilson, 15 of 20: 0.531 to 0.888.
        let (low, high) = wilson(15.0, 20.0);

        assert!((low - 0.531).abs() < 0.001 && (high - 0.888).abs() < 0.001);

        // Newcombe's own example: 56 of 70 against 48 of 80 gives
        // 0.0524 to 0.3339.
        let first: Vec<f64> = (0..70)
            .map(|index| f64::from(u8::from(index < 56)))
            .collect();
        let second: Vec<f64> = (0..80)
            .map(|index| f64::from(u8::from(index < 48)))
            .collect();

        let [low, high] = newcombe(&first, &second);

        assert!((low - 0.0524).abs() < 0.001, "{low}");
        assert!((high - 0.3339).abs() < 0.001, "{high}");

        // Student's t at 97.5%: 2.228 for 10 degrees of freedom, 2.086
        // for 20, 1.984 for 100.
        assert!((student_975(10.0) - 2.228).abs() < 0.002);
        assert!((student_975(20.0) - 2.086).abs() < 0.001);
        assert!((student_975(100.0) - 1.984).abs() < 0.001);

        // Welch: 1..=5 against 3..=7 differ by 2, give or take 2.306
        // standard errors of 1.
        let [low, high] = welch(&[1.0, 2.0, 3.0, 4.0, 5.0], &[3.0, 4.0, 5.0, 6.0, 7.0]).unwrap();

        assert!(
            (low + 4.306).abs() < 0.01 && (high - 0.306).abs() < 0.01,
            "{low} {high}"
        );

        assert_eq!(welch(&[1.0], &[2.0, 3.0]), None);
    }
}
