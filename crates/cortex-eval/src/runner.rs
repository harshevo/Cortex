//! Running the agent over a suite.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use cortex_core::trajectory::unix_millis;
use cortex_core::{
    Adviser, Agent, AgentConfig, AgentError, AutoApprove, ContextSource, Extras, JsonlSink,
    Message, ModelProvider, Progress, ProviderError, PruneConfig, RunReport, ToolContext,
    ToolRegistry, TrajectorySink, Verifier, Workspace, prompt,
};
use cortex_providers::pricing;

use crate::check;
use crate::report::{SuiteReport, TaskResult};
use crate::task::{Suite, Task};
use crate::workdir;

/// How often an attempt is carried on after the provider gave up on a
/// request. The provider has already retried for a long time by then, so
/// each of these is a fresh round of patience, not a quick second try.
pub const DEFAULT_MAX_RESUMES: u32 = 5;

/// Wait before the first resume. It doubles each time, up to
/// [`LONGEST_RESUME_PAUSE`].
pub const DEFAULT_RESUME_PAUSE: Duration = Duration::from_secs(60);

const LONGEST_RESUME_PAUSE: Duration = Duration::from_secs(900);

#[derive(Clone)]
pub struct RunOptions {
    /// Name for this configuration, for example `baseline` or `with-memory`.
    pub label: String,

    /// Directory that receives one subdirectory per run.
    pub results_dir: PathBuf,

    /// Step limit for tasks that do not set their own.
    pub max_steps: u32,

    /// The tools the agent may use.
    pub tools: ToolRegistry,

    /// Context pruning, when the configuration under test uses it.
    pub prune: Option<PruneConfig>,

    /// Attempts per task. Model output varies between runs, so one attempt
    /// is a weak measurement.
    pub repeats: u32,

    /// Consulted before each task: memory, or the router.
    pub context_sources: Vec<Arc<dyn ContextSource>>,

    /// Receive every event of every attempt, besides the trajectory file.
    /// Memory learns from a run this way.
    pub sinks: Vec<Arc<dyn TrajectorySink>>,

    /// The verification gate, when the configuration under test uses it.
    /// It runs the project's own checks, never the task's hidden ones.
    pub verifier: Option<Arc<dyn Verifier>>,

    pub max_verification_rounds: u32,

    /// May add a note after each step's tool results: the decision
    /// controllers.
    pub advisers: Vec<Arc<dyn Adviser>>,

    /// Paragraphs added to the system prompt of every attempt.
    pub prompt_notes: Vec<String>,

    /// Answer an unchanged repeated read with one line.
    pub fold_repeats: bool,

    /// Masking, the review, lessons and repeated attempts, as the
    /// configuration under test has them.
    pub extras: Extras,

    /// Times one attempt may be carried on after a provider failure
    /// before it is recorded as an error.
    pub max_resumes: u32,

    /// Wait before the first resume; doubled for each one after it.
    pub resume_pause: Duration,
}

impl RunOptions {
    /// The plain agent: the given tools and nothing else.
    pub fn plain(label: &str, results_dir: &Path, tools: ToolRegistry) -> Self {
        Self {
            label: label.to_string(),
            results_dir: results_dir.to_path_buf(),
            max_steps: 60,
            tools,
            prune: None,
            repeats: 1,
            context_sources: Vec::new(),
            sinks: Vec::new(),
            verifier: None,
            max_verification_rounds: 2,
            advisers: Vec::new(),
            prompt_notes: Vec::new(),
            fold_repeats: false,
            extras: Extras::default(),
            max_resumes: DEFAULT_MAX_RESUMES,
            resume_pause: DEFAULT_RESUME_PAUSE,
        }
    }
}

/// Runs every task `repeats` times, saves the report, and returns it.
///
/// `on_result` is called after each attempt so a caller can show progress.
pub async fn run_suite(
    suite: &Suite,
    provider: Arc<dyn ModelProvider>,
    options: &RunOptions,
    on_result: impl Fn(&TaskResult),
) -> Result<(SuiteReport, PathBuf)> {
    let created = unix_millis() as u64;

    let run_id = format!("{}-{}", options.label, created / 1000);
    let run_dir = options.results_dir.join(&run_id);

    let mut report = SuiteReport {
        run_id,
        label: options.label.clone(),
        suite: suite.name.clone(),
        provider: provider.name().to_string(),
        model: provider.model().to_string(),
        created_unix_ms: created,
        results: Vec::new(),
    };

    for task in &suite.tasks {
        for attempt in 1..=options.repeats {
            let result = run_attempt(task, attempt, provider.clone(), &run_dir, options).await?;

            on_result(&result);

            report.results.push(result);

            // Save after every attempt so an interrupted run keeps what it
            // has measured so far.
            report.save(&run_dir)?;
        }
    }

    Ok((report, run_dir))
}

async fn run_attempt(
    task: &Task,
    attempt: u32,
    provider: Arc<dyn ModelProvider>,
    run_dir: &Path,
    options: &RunOptions,
) -> Result<TaskResult> {
    let workdir = workdir::prepare(task)?;
    let workspace = Workspace::new(workdir.path())?;

    let trajectory_file = run_dir
        .join("trajectories")
        .join(format!("{}-{attempt}.jsonl", task.id));

    let sink = JsonlSink::create(&trajectory_file)?;

    let mut notes = options.prompt_notes.clone();

    notes.extend(
        options
            .verifier
            .as_ref()
            .and_then(|verifier| prompt::verification_note(&verifier.commands(&workspace))),
    );

    let config = AgentConfig {
        system_prompt: prompt::system_prompt_with(&workspace, &notes),
        max_steps: task.max_steps.unwrap_or(options.max_steps),
        prune: options.prune.clone(),
        max_verification_rounds: options.max_verification_rounds,
        fold_repeats: options.fold_repeats,
        extras: options.extras.clone(),
    };

    // Auto-approval is safe here only because the working copy is disposable.
    let context = ToolContext {
        workspace,
        approver: Arc::new(AutoApprove),
    };

    let model = provider.model().to_string();

    let mut agent =
        Agent::new(provider, options.tools.clone(), context, config).with_sink(Arc::new(sink));

    for sink in &options.sinks {
        agent = agent.with_sink(sink.clone());
    }

    for source in &options.context_sources {
        agent = agent.with_context_source(source.clone());
    }

    if let Some(verifier) = &options.verifier {
        agent = agent.with_verifier(verifier.clone());
    }

    for adviser in &options.advisers {
        agent = agent.with_adviser(adviser.clone());
    }

    let mut conversation = Vec::new();

    let finished = run_to_the_end(&agent, &mut conversation, task, options).await;

    let (run, resumes) = match finished {
        Finished::Run { run, resumes } => (run, resumes),

        // A rejected key or account will fail every remaining attempt the
        // same way, so stop the whole run instead of recording noise.
        Finished::Failed { error, .. } if error.is_account_problem() => {
            return Err(anyhow!(error).context("the provider rejected the request; stopping"));
        }

        // What the attempt used before it failed was really spent, so it
        // is recorded. The report keeps it out of the per-attempt means.
        Finished::Failed {
            error,
            progress,
            resumes,
        } => {
            return Ok(TaskResult {
                task_id: task.id.clone(),
                attempt,
                solved: false,
                outcome: None,
                error: Some(error.to_string()),
                steps: progress.steps,
                usage: progress.usage,
                cost_usd: pricing::cost_usd(&model, &progress.usage),
                duration_ms: progress.elapsed.as_millis() as u64,
                verified: None,
                resumes,
                milestones: Vec::new(),
            });
        }
    };

    let check = check::run(task, workdir.path()).await?;

    Ok(TaskResult {
        task_id: task.id.clone(),
        attempt,
        solved: check.passed,
        outcome: Some(run.outcome),
        error: None,
        steps: run.steps,
        usage: run.usage,
        cost_usd: pricing::cost_usd(&model, &run.usage),
        duration_ms: run.duration.as_millis() as u64,
        verified: run.verified,
        resumes,
        milestones: check.milestones,
    })
}

/// How one attempt ended.
enum Finished {
    Run {
        run: RunReport,
        resumes: u32,
    },

    Failed {
        error: ProviderError,
        progress: Progress,
        resumes: u32,
    },
}

/// Runs the task, carrying on from where it stopped each time the
/// provider gives up on a request.
///
/// A run loses nothing when a request fails: the conversation still holds
/// every finished step. Starting the attempt again would pay for all of
/// them a second time, so the failed step alone is asked again after a
/// wait.
async fn run_to_the_end(
    agent: &Agent,
    conversation: &mut Vec<Message>,
    task: &Task,
    options: &RunOptions,
) -> Finished {
    let mut resumes = 0;

    let mut outcome = agent.run(conversation, &task.instruction).await;

    loop {
        let (error, progress) = match outcome {
            Ok(run) => return Finished::Run { run, resumes },
            Err(AgentError::Provider { error, progress }) => (error, progress),
        };

        if !error.is_retryable() || resumes >= options.max_resumes {
            return Finished::Failed {
                error,
                progress,
                resumes,
            };
        }

        let pause = resume_pause(options.resume_pause, resumes);

        eprintln!(
            "  {}: the provider failed after {} steps ({error}); carrying on in {} s",
            task.id,
            progress.steps,
            pause.as_secs(),
        );

        tokio::time::sleep(pause).await;

        resumes += 1;

        outcome = agent.resume(conversation, progress).await;
    }
}

/// The first pause, doubled for each resume already made.
fn resume_pause(first: Duration, resumes: u32) -> Duration {
    first
        .saturating_mul(2u32.saturating_pow(resumes))
        .min(LONGEST_RESUME_PAUSE)
}
