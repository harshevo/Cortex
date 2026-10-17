//! Several attempts at one task, of which the best is kept.
//!
//! Each attempt works in its own copy of the project, so attempts cannot
//! disturb each other or the real files. When they are done, each one's
//! changes are judged twice: by the project's checks, which the loop has
//! already run, and by a review of the diff against the request. The
//! changes of the best attempt are copied back; the rest are thrown away.
//!
//! By default the attempts are made one after another and the first
//! convincing one ends the search, so a task the first attempt solves
//! costs one attempt and one review. Made at the same time they finish
//! sooner but always cost all of them.
//!
//! Each attempt after the first is given a different way of going about
//! the task. Which ways are tried is decided by a [`StrategyPolicy`], which
//! is told afterwards which of them produced convincing work.

use std::fmt::Debug;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde_json::json;
use tempfile::TempDir;
use tokio::process::Command;
use tokio::task::JoinSet;

use crate::agent::{Agent, AgentConfig, Extras, Outcome, Progress, RunReport, last_assistant_text};
use crate::briefing::{self, Briefing};
use crate::changes::{self, Change};
use crate::error::{AgentError, ProviderError};
use crate::message::{Image, Message};
use crate::review::{self, Review};
use crate::sandbox::Sandbox;
use crate::tool::ToolContext;
use crate::trajectory::{Event, TrajectorySink};
use crate::usage::Usage;
use crate::workspace::Workspace;

/// The diff shown to the reviewer is shortened to this.
const MAX_REVIEWED_DIFF_CHARS: usize = 24_000;

/// A way of going about a task, given to one attempt.
#[derive(Debug, Clone, PartialEq)]
pub struct Strategy {
    pub name: String,

    /// Shown to the model in front of the task. Empty for the plain
    /// attempt.
    pub note: String,
}

impl Strategy {
    pub fn new(name: &str, note: &str) -> Self {
        Self {
            name: name.to_string(),
            note: note.to_string(),
        }
    }
}

/// The strategies an attempt can be given. The first is the plain agent.
pub fn strategies() -> Vec<Strategy> {
    vec![
        Strategy::new("plain", ""),
        Strategy::new(
            "reproduce_first",
            "Before changing any source file, write a small test or script that shows the \
             problem, run it, and see it fail. Then fix the cause and run it again.",
        ),
        Strategy::new(
            "read_widely",
            "Before the first edit, find every place that touches the behaviour in the task: \
             the definition, each caller, and the tests. Fix the cause where it is shared, \
             not one symptom.",
        ),
        Strategy::new(
            "smallest_patch",
            "Aim for the smallest change that fully solves the task. Before finishing, read \
             your own diff and take out anything the task does not need.",
        ),
    ]
}

/// Chooses which strategies the attempts of a run are given, and learns
/// from how they went.
pub trait StrategyPolicy: Send + Sync + Debug {
    /// `count` strategies, in the order the attempts are made.
    fn choose(&self, count: usize) -> Vec<Strategy>;

    /// Told after a run which strategies were tried and whether each
    /// produced convincing work.
    fn learn(&self, tried: &[(String, bool)]);
}

/// Always the same strategies in the same order.
#[derive(Debug, Default)]
pub struct FixedStrategies;

impl StrategyPolicy for FixedStrategies {
    fn choose(&self, count: usize) -> Vec<Strategy> {
        strategies().into_iter().cycle().take(count).collect()
    }

    fn learn(&self, _tried: &[(String, bool)]) {}
}

#[derive(Debug, Clone)]
pub struct RolloutConfig {
    /// Attempts made at most.
    pub count: u32,

    /// Make all attempts at the same time instead of stopping at the
    /// first convincing one.
    pub parallel: bool,

    pub policy: Arc<dyn StrategyPolicy>,
}

impl RolloutConfig {
    pub fn new(count: u32) -> Self {
        Self {
            count,
            parallel: false,
            policy: Arc::new(FixedStrategies),
        }
    }
}

/// One attempt and everything known about it.
struct Candidate {
    attempt: u32,
    strategy: Strategy,

    /// The copy it worked in. Dropping it deletes the copy.
    copy: Option<TempDir>,

    conversation: Vec<Message>,
    events: Vec<Event>,
    outcome: Option<Outcome>,
    steps: u32,
    usage: Usage,
    verified: Option<bool>,
    changes: Vec<Change>,
    review: Option<Review>,
    provider_error: Option<ProviderError>,
    error: Option<String>,
    duration_ms: u64,
}

impl Candidate {
    /// Work that passed what it was held against: it finished, no check
    /// failed, and the review found nothing unmet.
    fn is_convincing(&self) -> bool {
        self.outcome == Some(Outcome::Completed)
            && self.verified != Some(false)
            && self
                .review
                .as_ref()
                .is_none_or(|review| review.unmet().next().is_none())
    }

    /// Higher is better. Checks count first, then the review, then a
    /// smaller change.
    fn rank(&self) -> (u8, u8, i64, i64) {
        let finished = u8::from(self.outcome == Some(Outcome::Completed));

        let checks = match self.verified {
            Some(true) => 2,
            None => 1,
            Some(false) => 0,
        };

        let review = self.review.as_ref().map_or(0.5, Review::score);

        (
            u8::from(self.outcome.is_some()) + finished,
            checks,
            (review * 1_000.0) as i64,
            -(changes::size(&self.changes) as i64),
        )
    }

    fn finished_event(&self) -> Event {
        Event::AttemptFinished {
            attempt: self.attempt,
            strategy: self.strategy.name.clone(),
            outcome: self.outcome,
            steps: self.steps,
            usage: self.usage,
            verified: self.verified,
            review_score: self.review.as_ref().map(Review::score),
            changed_files: self.changes.len(),
            error: self.error.clone(),
            duration_ms: self.duration_ms,
        }
    }
}

/// Keeps the events of an attempt, to be replayed if it is the one kept.
#[derive(Default)]
struct Collector {
    events: Mutex<Vec<Event>>,
}

impl TrajectorySink for Collector {
    fn record(&self, event: &Event) {
        self.events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(event.clone());
    }
}

impl Agent {
    pub(crate) async fn run_rollouts(
        &self,
        rollouts: &RolloutConfig,
        conversation: &mut Vec<Message>,
        instruction: &str,
        images: &[Image],
        briefings: Vec<Briefing>,
    ) -> Result<RunReport, AgentError> {
        let started = Instant::now();

        let strategies = rollouts.policy.choose(rollouts.count as usize);
        let of = strategies.len() as u32;

        let plan = Plan {
            root: self.context.workspace.root().to_path_buf(),
            sandbox: self.context.workspace.sandbox(),
            instruction: instruction.to_string(),
            images: images.to_vec(),
            briefings: briefings.clone(),
        };

        let mut candidates = Vec::new();

        if rollouts.parallel {
            let mut tasks = JoinSet::new();

            for (position, strategy) in strategies.into_iter().enumerate() {
                let attempt = position as u32 + 1;

                self.emit(Event::AttemptStarted {
                    attempt,
                    of,
                    strategy: strategy.name.clone(),
                });

                tasks.spawn(make_attempt(self.fork(), plan.clone(), attempt, strategy));
            }

            while let Some(joined) = tasks.join_next().await {
                let Ok(candidate) = joined else {
                    continue;
                };

                self.emit(candidate.finished_event());

                candidates.push(candidate);
            }

            candidates.sort_by_key(|candidate| candidate.attempt);
        } else {
            for (position, strategy) in strategies.into_iter().enumerate() {
                let attempt = position as u32 + 1;

                self.emit(Event::AttemptStarted {
                    attempt,
                    of,
                    strategy: strategy.name.clone(),
                });

                let candidate = make_attempt(self.fork(), plan.clone(), attempt, strategy).await;

                self.emit(candidate.finished_event());

                let stop = candidate.is_convincing()
                    // A rejected key fails every attempt the same way.
                    || candidate
                        .provider_error
                        .as_ref()
                        .is_some_and(|error| !error.is_retryable());

                candidates.push(candidate);

                if stop {
                    break;
                }
            }
        }

        let mut progress = Progress::starting(instruction);

        for candidate in &candidates {
            progress.steps += candidate.steps;
            progress.usage += candidate.usage;
        }

        let tried: Vec<(String, bool)> = candidates
            .iter()
            .map(|candidate| (candidate.strategy.name.clone(), candidate.is_convincing()))
            .collect();

        let made = candidates.len() as u32;

        let best = candidates
            .into_iter()
            .filter(|candidate| candidate.outcome.is_some())
            .max_by_key(|candidate| (candidate.rank(), std::cmp::Reverse(candidate.attempt)));

        let Some(winner) = best else {
            // No attempt got as far as an answer. Leave the conversation
            // as a plain run would have it, so the caller can resume.
            conversation.push(Message::user_with_images(
                briefing::compose(&briefings, instruction),
                images,
            ));

            progress.elapsed = started.elapsed();

            let error = ProviderError::Network(
                "no attempt could be made (the project could not be copied, or the provider \
                 failed every time)"
                    .to_string(),
            );

            return Err(AgentError::Provider { error, progress });
        };

        rollouts.policy.learn(&tried);

        let mut reason = describe_choice(&winner, made);

        if let Some(copy) = &winner.copy
            && let Err(error) = changes::apply(&winner.changes, copy.path(), &plan.root)
        {
            reason.push_str(&format!("; its changes could not be copied back: {error}"));
        }

        self.emit(Event::AttemptChosen {
            attempt: winner.attempt,
            of: made,
            reason,
        });

        // From here the log reads as if the kept attempt had been the
        // only one, which is also what memory should learn from.
        if !winner.strategy.note.is_empty() {
            self.emit(Event::ContextPrepared {
                source: "rollout".to_string(),
                text: strategy_briefing(&winner.strategy).text,
                detail: json!({ "strategy": winner.strategy.name }),
                duration_ms: 0,
            });
        }

        for event in winner.events {
            self.emit(event);
        }

        conversation.extend(winner.conversation);

        progress.gate.changed = !winner.changes.is_empty();
        progress.gate.verified = winner.verified;
        progress.elapsed = started.elapsed();

        let outcome = winner.outcome.unwrap_or(Outcome::Completed);

        Ok(self.finish(conversation, outcome, progress).await)
    }

    /// An agent like this one, for an attempt: the same model, tools and
    /// checks, and none of what watches or steers a single run.
    fn fork(&self) -> Agent {
        let config = AgentConfig {
            extras: Extras {
                mask: self.config.extras.mask.clone(),
                compact: self.config.extras.compact.clone(),
                context_window: self.config.extras.context_window,
                ..Extras::default()
            },
            ..self.config.clone()
        };

        Agent {
            provider: self.provider.clone(),
            tools: self.tools.clone(),
            context: self.context.clone(),
            config,
            sinks: Vec::new(),
            context_sources: Vec::new(),
            verifier: self.verifier.clone(),
            advisers: Vec::new(),
            stream_observer: None,
            inbox: None,
            heard: Arc::default(),
            helper: self.helper.clone(),
            hooks: self.hooks.clone(),
            escalation: None,
            recovery: None,
            decisions: self.decisions.clone(),
        }
    }
}

/// What every attempt of a run starts from.
#[derive(Clone)]
struct Plan {
    /// The real project.
    root: PathBuf,

    /// Whether the real project's commands are confined; a copy's are too.
    sandbox: Sandbox,

    instruction: String,
    images: Vec<Image>,
    briefings: Vec<Briefing>,
}

/// Copies the project, runs the task in the copy, and judges the result.
async fn make_attempt(mut agent: Agent, plan: Plan, attempt: u32, strategy: Strategy) -> Candidate {
    let started = Instant::now();

    let mut candidate = Candidate {
        attempt,
        strategy,
        copy: None,
        conversation: Vec::new(),
        events: Vec::new(),
        outcome: None,
        steps: 0,
        usage: Usage::default(),
        verified: None,
        changes: Vec::new(),
        review: None,
        provider_error: None,
        error: None,
        duration_ms: 0,
    };

    let (copy, workspace) = match copy_project(&plan.root, plan.sandbox).await {
        Ok(copied) => copied,

        Err(error) => {
            candidate.error = Some(format!("the project could not be copied: {error}"));

            return candidate;
        }
    };

    // The prompt names the project directory. An attempt that was told
    // the real one could reach it with an absolute path.
    agent.config.system_prompt = agent.config.system_prompt.replace(
        &plan.root.display().to_string(),
        &workspace.root().display().to_string(),
    );

    agent.context = ToolContext {
        workspace: workspace.clone(),
        approver: agent.context.approver.clone(),
    };

    let collector = Arc::new(Collector::default());

    agent.sinks.push(collector.clone());

    let mut briefings = plan.briefings.clone();

    if !candidate.strategy.note.is_empty() {
        briefings.push(strategy_briefing(&candidate.strategy));
    }

    candidate.conversation = vec![Message::user_with_images(
        briefing::compose(&briefings, &plan.instruction),
        &plan.images,
    )];

    let worked = agent
        .steps(
            &mut candidate.conversation,
            Progress::starting(&plan.instruction),
        )
        .await;

    match worked {
        Ok((outcome, progress)) => {
            candidate.outcome = Some(outcome);
            candidate.steps = progress.steps;
            candidate.usage = progress.usage;
            candidate.verified = progress.gate.verified;
        }

        Err(AgentError::Provider { error, progress }) => {
            candidate.steps = progress.steps;
            candidate.usage = progress.usage;
            candidate.error = Some(error.to_string());
            candidate.provider_error = Some(error);
        }
    }

    candidate.events = std::mem::take(
        &mut *collector
            .events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    );

    candidate.changes = changes::between(&plan.root, workspace.root());

    // Work whose checks failed is ranked last whatever a review says, so
    // it is not worth the call.
    let worth_reviewing = candidate.outcome == Some(Outcome::Completed)
        && candidate.verified != Some(false)
        && !candidate.changes.is_empty();

    if worth_reviewing {
        let diff = changes::render(&candidate.changes, MAX_REVIEWED_DIFF_CHARS);
        let report = last_assistant_text(&candidate.conversation);

        let reviewed = review::review(agent.helper(), &plan.instruction, &diff, &report).await;

        if let Ok(Some(review)) = reviewed {
            candidate.usage += review.usage;
            candidate.review = Some(review);
        }
    }

    candidate.copy = Some(copy);
    candidate.duration_ms = started.elapsed().as_millis() as u64;

    candidate
}

fn strategy_briefing(strategy: &Strategy) -> Briefing {
    Briefing {
        source: "rollout".to_string(),
        text: format!("## How to go about this task\n\n{}\n", strategy.note),
        detail: json!({ "strategy": strategy.name }),
    }
}

fn describe_choice(winner: &Candidate, made: u32) -> String {
    let checks = match winner.verified {
        Some(true) => "its checks passed",
        Some(false) => "its checks failed, as did every other attempt's",
        None => "no checks were run",
    };

    let review = match &winner.review {
        Some(review) => format!(
            "the review found {} of {} requirements met",
            review.requirements.len() - review.unmet().count(),
            review.requirements.len()
        ),

        None => "it was not reviewed".to_string(),
    };

    if made == 1 {
        format!("the first attempt was convincing: {checks}, {review}")
    } else {
        format!("best of {made}: {checks}, {review}")
    }
}

/// A copy of the project in a temporary directory.
///
/// On macOS the files are cloned, which takes no extra disk space until
/// one of them is changed. Links are copied as links.
pub async fn copy_project(root: &Path, sandbox: Sandbox) -> io::Result<(TempDir, Workspace)> {
    let copy = tempfile::Builder::new()
        .prefix("cortex-attempt-")
        .tempdir()?;

    let source = root.join(".");

    let cloned = cfg!(target_os = "macos") && copy_with(&["-Rc"], &source, copy.path()).await?;

    if !cloned {
        // Windows has no `cp`.
        let copied = cfg!(unix) && copy_with(&["-R"], &source, copy.path()).await?;

        if !copied {
            let (from, to) = (root.to_path_buf(), copy.path().to_path_buf());

            tokio::task::spawn_blocking(move || copy_tree(&from, &to))
                .await
                .map_err(io::Error::other)??;
        }
    }

    let workspace = Workspace::new(copy.path())?.with_sandbox(sandbox);

    Ok((copy, workspace))
}

/// Copies everything in `source` into `target`, which exists.
fn copy_tree(source: &Path, target: &Path) -> io::Result<()> {
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let to = target.join(entry.file_name());

        if entry.file_type()?.is_dir() {
            std::fs::create_dir_all(&to)?;

            copy_tree(&entry.path(), &to)?;
        } else {
            // A link that points nowhere is not worth failing the copy.
            let _ = std::fs::copy(entry.path(), &to);
        }
    }

    Ok(())
}

async fn copy_with(flags: &[&str], source: &Path, target: &Path) -> io::Result<bool> {
    let status = Command::new("cp")
        .args(flags)
        .arg(source)
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?;

    Ok(status.success())
}
