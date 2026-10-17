//! The job runner: `cortex jobs serve`.
//!
//! One runner looks after a project's jobs at a time. Once a second it
//! says it is alive, and:
//!
//! 1. takes in the jobs its agents have finished: commits each one's work
//!    on its branch, and records how it went;
//! 2. stops the jobs someone asked to stop;
//! 3. takes up jobs a runner that died left running, from their task
//!    records ([`cortex_core::task_state`]);
//! 4. every few seconds, works out which files each job has changed, and
//!    tells running jobs which other jobs are changing the same files;
//! 5. merges at most one finished job's branch into the project, after a
//!    trial merge, so merges happen one at a time in the order the jobs
//!    were queued;
//! 6. starts queued jobs whose dependencies are merged, up to the number
//!    it may run at once.
//!
//! It leaves when it has had nothing to do for a while, and is started
//! again by whatever queues the next job. It runs apart from any window,
//! so closing the window that started a job does not stop it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Parser;
use cortex_core::jobs::{
    self, DEFAULT_PARALLEL, Job, JobStatus, JobStore, Readiness, RunnerConfig,
};
use cortex_core::task_state::{TaskState, TaskStateSink};
use cortex_core::{AgentError, Event, Inbox, Outcome, TrajectorySink, merge, worktree};
use tokio::task::JoinHandle;

use super::approver::JobApprover;
use crate::args::{JobsServeArgs, ModelArgs, SessionArgs};
use crate::session::{BuildOptions, build_agent_with};

/// How often the runner looks at its jobs.
const TICK: Duration = Duration::from_secs(1);

/// How often the files each job has changed are worked out again.
const TRACK_EVERY: Duration = Duration::from_secs(3);

/// How long a merge that had to wait is left before it is tried again.
const MERGE_RETRY: Duration = Duration::from_secs(15);

/// Lets the `cortex` command's model arguments be parsed on their own.
#[derive(Parser)]
struct ModelOnly {
    #[command(flatten)]
    model: ModelArgs,
}

/// Parses `cortex run` model arguments, such as `--provider local`.
pub fn parse_model_args(arguments: &[String]) -> Result<ModelArgs> {
    let argv = std::iter::once("cortex".to_string()).chain(arguments.iter().cloned());

    ModelOnly::try_parse_from(argv)
        .map(|parsed| parsed.model)
        .context("the runner's model arguments are not ones `cortex run` takes")
}

pub async fn serve(args: JobsServeArgs) -> Result<ExitCode> {
    let store = JobStore::for_project(&args.dir);

    let saved = store.config();

    let config = match (args.model.is_empty(), saved) {
        (false, saved) => RunnerConfig {
            model_args: args.model.clone(),
            max_parallel: args
                .parallel
                .or(saved.map(|saved| saved.max_parallel))
                .unwrap_or(DEFAULT_PARALLEL),
        },

        (true, Some(saved)) => RunnerConfig {
            max_parallel: args.parallel.unwrap_or(saved.max_parallel),
            ..saved
        },

        (true, None) => RunnerConfig {
            model_args: Vec::new(),
            max_parallel: args.parallel.unwrap_or(DEFAULT_PARALLEL),
        },
    };

    let model = parse_model_args(&config.model_args)?;

    store.save_config(&config).map_err(anyhow::Error::msg)?;

    let pid = std::process::id();

    if !store.claim_runner(pid).map_err(anyhow::Error::msg)? {
        eprintln!("Another runner is carrying out this project's jobs.");

        return Ok(ExitCode::SUCCESS);
    }

    eprintln!(
        "Runner {pid} for {}: up to {} jobs at once.",
        store.root().display(),
        config.max_parallel.max(1)
    );

    let mut runner = Runner {
        git: merge::has_history(store.root()).await,
        store,
        model,
        parallel: config.max_parallel.max(1),
        idle_exit: Duration::from_secs(args.idle_exit),
        pid,
        active: BTreeMap::new(),
        merge_tried: BTreeMap::new(),
        told: BTreeSet::new(),
    };

    let result = runner.run().await;

    runner.store.release_runner(pid);

    result.map(|()| ExitCode::SUCCESS)
}

struct Runner {
    store: JobStore,
    model: ModelArgs,
    parallel: usize,
    idle_exit: Duration,
    pid: u32,

    /// The project is a repository with a commit, so jobs get worktrees.
    git: bool,

    /// The jobs this runner's agents are carrying out.
    active: BTreeMap<String, Active>,

    /// When each job's merge last had to wait.
    merge_tried: BTreeMap<String, Instant>,

    /// Overlaps already told to running jobs: (job, other job, file).
    told: BTreeSet<(String, String, String)>,
}

struct Active {
    task: JoinHandle<Result<Report, String>>,
    inbox: Arc<Inbox>,
    mode: Mode,
    counts: Arc<Mutex<Counts>>,
}

#[derive(Clone, PartialEq)]
enum Mode {
    /// Carrying out the job's instruction.
    Work,

    /// Resolving the conflicts of its branch.
    Resolve,
}

#[derive(Default, Clone, Copy, PartialEq)]
struct Counts {
    steps: u32,
    tokens: u64,
}

struct Report {
    outcome: Outcome,
    verified: Option<bool>,
    steps: u32,
    tokens: u64,
    summary: String,
}

impl Runner {
    async fn run(&mut self) -> Result<()> {
        let mut tracked = Instant::now() - TRACK_EVERY;
        let mut idle_since: Option<Instant> = None;

        loop {
            if let Err(error) = self.store.beat(self.pid) {
                // Another runner holds the project now; it takes up the
                // jobs left running from their records.
                eprintln!("Leaving: {error}.");

                for (_, active) in std::mem::take(&mut self.active) {
                    active.task.abort();
                }

                return Ok(());
            }

            self.take_in_finished().await;

            let all = self.store.list();

            self.stop_and_adopt(&all).await;
            self.report_counts();

            if tracked.elapsed() >= TRACK_EVERY {
                tracked = Instant::now();

                self.track_changes().await;
            }

            let all = self.store.list();
            let merging = self.merge_one(&all).await;

            let all = self.store.list();
            let started = self.start_ready(&all).await;

            let busy = merging || started || !self.active.is_empty();

            match (busy, idle_since) {
                (true, _) => idle_since = None,
                (false, None) => idle_since = Some(Instant::now()),
                (false, Some(since)) if since.elapsed() >= self.idle_exit => {
                    eprintln!("Nothing left to do.");

                    return Ok(());
                }
                (false, Some(_)) => {}
            }

            tokio::time::sleep(TICK).await;
        }
    }

    /// Records how each finished agent did, after committing its work.
    async fn take_in_finished(&mut self) {
        let finished: Vec<String> = self
            .active
            .iter()
            .filter(|(_, active)| active.task.is_finished())
            .map(|(id, _)| id.clone())
            .collect();

        for id in finished {
            let Some(active) = self.active.remove(&id) else {
                continue;
            };

            let result = match active.task.await {
                Ok(result) => result,
                Err(error) if error.is_cancelled() => Err("it was stopped".to_string()),
                Err(_) => Err("its agent stopped unexpectedly".to_string()),
            };

            let Some(job) = self.store.get(&id) else {
                continue;
            };

            let changes = self.commit(&job).await;

            match active.mode {
                Mode::Work => self.after_work(&job, result, changes),
                Mode::Resolve => self.after_resolving(&job, result, changes).await,
            }
        }
    }

    /// Commits the job's work on its branch, and says which files it has
    /// changed since it started.
    async fn commit(&self, job: &Job) -> Result<Vec<String>, String> {
        let (Some(worktree), Some(base)) = (&job.worktree, &job.base) else {
            return Ok(Vec::new());
        };

        // A merge left open is concluded by the resolution, not here.
        if !merge::in_progress(worktree).await {
            merge::commit_all(worktree, &format!("{}: {}", job.id, job.title)).await?;
        }

        merge::changed_files(worktree, base).await
    }

    fn after_work(&self, job: &Job, result: Result<Report, String>, changes: Result<Vec<String>, String>) {
        let now = jobs::now_ms();

        let _ = self.store.update(&job.id, |job| {
            job.finished_ms = Some(now);

            if let Ok(files) = &changes {
                job.changed_files = files.clone();
            }

            match result {
                Ok(report) => {
                    job.steps = report.steps;
                    job.tokens = report.tokens;
                    job.verified = report.verified;
                    job.summary = report.summary;
                    job.outcome = Some(jobs::outcome_label(report.outcome));

                    match report.outcome {
                        Outcome::Completed => {
                            job.status = JobStatus::Done;
                            job.error = match (report.verified, job.branch.is_some()) {
                                (Some(false), true) => Some(
                                    "the project's checks failed; it is not merged on its \
                                     own, merge it if the work is right"
                                        .to_string(),
                                ),
                                _ => None,
                            };
                        }

                        other => {
                            job.status = JobStatus::Failed;
                            job.error = Some(format!(
                                "the agent stopped: {}",
                                jobs::outcome_label(other)
                            ));
                        }
                    }
                }

                Err(error) => {
                    job.status = JobStatus::Failed;
                    job.error = Some(error);
                }
            }

            if let Err(error) = &changes {
                job.error = Some(format!("its work could not be committed: {error}"));
                job.status = JobStatus::Failed;
            }
        });
    }

    async fn after_resolving(
        &self,
        job: &Job,
        result: Result<Report, String>,
        changes: Result<Vec<String>, String>,
    ) {
        let left = match (&result, &job.worktree) {
            (Ok(_), Some(worktree)) => merge::finish_merge(worktree).await,
            (Ok(_), None) => Err("the job has no worktree".to_string()),
            (Err(error), _) => Err(error.clone()),
        };

        let changes = match &left {
            Ok(files) if files.is_empty() => self.commit(job).await,
            _ => changes,
        };

        let now = jobs::now_ms();

        let _ = self.store.update(&job.id, |job| {
            job.finished_ms = Some(now);
            job.resolve_requested = false;

            if let Ok(files) = changes {
                job.changed_files = files;
            }

            if let Ok(report) = &result {
                job.steps += report.steps;
                job.tokens += report.tokens;
            }

            match left {
                Ok(files) if files.is_empty() => {
                    job.status = JobStatus::Done;
                    job.conflicts.clear();
                    job.error = None;

                    // Resolving was asked for so that it could be merged.
                    job.merge_requested = true;
                }

                Ok(files) => {
                    job.status = JobStatus::Conflicted;
                    job.error = Some("conflict markers are left in its worktree".to_string());
                    job.conflicts = files;
                }

                Err(error) => {
                    job.status = JobStatus::Conflicted;
                    job.error = Some(format!("the conflicts were not resolved: {error}"));
                }
            }
        });
    }

    /// Stops what was asked to stop, and takes up jobs left running by a
    /// runner that is gone.
    async fn stop_and_adopt(&mut self, all: &[Job]) {
        for job in all.iter().filter(|job| job.status == JobStatus::Running) {
            let ours = self.active.contains_key(&job.id);

            if job.cancel_requested {
                if let Some(active) = self.active.remove(&job.id) {
                    active.task.abort();

                    let _ = active.task.await;
                }

                let now = jobs::now_ms();

                let _ = self.store.update(&job.id, |job| {
                    job.status = JobStatus::Cancelled;
                    job.cancel_requested = false;
                    job.finished_ms = Some(now);
                    job.error = Some("stopped on request; retrying carries on from here".into());
                });

                continue;
            }

            if !ours {
                let _ = self.store.update(&job.id, |job| {
                    job.status = JobStatus::Queued;
                    job.resumed += 1;
                    job.error = Some("its runner stopped; it carries on from its record".into());
                });
            }
        }
    }

    /// Copies each running job's steps and tokens into its record.
    fn report_counts(&self) {
        for (id, active) in &self.active {
            let counts = *active
                .counts
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());

            let Some(job) = self.store.get(id) else {
                continue;
            };

            let total = match active.mode {
                Mode::Work => counts,
                Mode::Resolve => Counts {
                    steps: job.steps,
                    tokens: job.tokens,
                },
            };

            if job.steps != total.steps || job.tokens != total.tokens {
                let _ = self.store.update(id, |job| {
                    job.steps = total.steps;
                    job.tokens = total.tokens;
                    job.heartbeat_ms = Some(jobs::now_ms());
                });
            }
        }
    }

    /// Works out the files each job with a worktree has changed, which
    /// jobs share files, and tells the running ones of new overlaps.
    async fn track_changes(&mut self) {
        let watched: Vec<Job> = self
            .store
            .list()
            .into_iter()
            .filter(|job| {
                matches!(
                    job.status,
                    JobStatus::Running | JobStatus::Done | JobStatus::Conflicted | JobStatus::Queued
                ) && job.worktree.as_ref().is_some_and(|path| path.exists())
                    && job.base.is_some()
            })
            .collect();

        let mut changes = Vec::new();

        for job in &watched {
            let (Some(worktree), Some(base)) = (&job.worktree, &job.base) else {
                continue;
            };

            if let Ok(files) = merge::changed_files(worktree, base).await {
                changes.push((job.id.clone(), files));
            }
        }

        let overlaps = merge::overlaps(&changes);

        for (id, files) in &changes {
            let found = overlaps.get(id).cloned().unwrap_or_default();

            let current = watched.iter().find(|job| &job.id == id);

            if current.is_some_and(|job| job.changed_files == *files && job.overlaps == found) {
                continue;
            }

            let _ = self.store.update(id, |job| {
                job.changed_files = files.clone();
                job.overlaps = found.clone();
            });
        }

        for (id, active) in &self.active {
            if active.mode != Mode::Work {
                continue;
            }

            for overlap in overlaps.get(id).into_iter().flatten() {
                let new: Vec<&String> = overlap
                    .files
                    .iter()
                    .filter(|file| {
                        self.told
                            .insert((id.clone(), overlap.job.clone(), (*file).clone()))
                    })
                    .collect();

                if new.is_empty() {
                    continue;
                }

                let title = watched
                    .iter()
                    .find(|job| job.id == overlap.job)
                    .map(|job| job.title.clone())
                    .unwrap_or_default();

                let files: Vec<&str> = new.iter().map(|file| file.as_str()).collect();

                active.inbox.push(format!(
                    "[From the job runner, not the person] {} (\"{title}\") is also changing {}. \
                     Keep your changes to these files to what your task needs; the two \
                     branches are merged one after the other, and overlapping edits can \
                     conflict.",
                    overlap.job,
                    files.join(", ")
                ));
            }
        }
    }

    /// Merges the first finished job that should be merged. Returns
    /// whether one was tried.
    async fn merge_one(&mut self, all: &[Job]) -> bool {
        let wanted = |job: &&Job| {
            job.status == JobStatus::Done
                && job.branch.is_some()
                && !self.active.contains_key(&job.id)
                && (job.merge_requested
                    || (job.auto_merge
                        && job.verified != Some(false)
                        && job.outcome.as_deref() == Some("completed")))
        };

        let due = |job: &&Job| {
            job.merge_requested
                || self
                    .merge_tried
                    .get(&job.id)
                    .is_none_or(|tried| tried.elapsed() >= MERGE_RETRY)
        };

        let Some(job) = all.iter().filter(wanted).find(due) else {
            return false;
        };

        let branch = job.branch.clone().unwrap_or_default();
        let message = format!("Merge {}: {}", job.id, job.title);

        let merged = merge::merge(self.store.root(), &branch, &message).await;

        match merged {
            Ok(merge::Merged::Done(commit)) => {
                self.merged(job, Some(commit)).await;
            }

            Ok(merge::Merged::AlreadyIn) => {
                self.merged(job, None).await;
            }

            Ok(merge::Merged::Conflicts(files)) => {
                let _ = self.store.update(&job.id, |job| {
                    job.status = JobStatus::Conflicted;
                    job.merge_requested = false;
                    job.error = Some(format!(
                        "merging conflicts with the project in {}",
                        files.join(", ")
                    ));
                    job.conflicts = files;
                });
            }

            Ok(merge::Merged::Waiting(reason)) | Err(reason) => {
                self.merge_tried.insert(job.id.clone(), Instant::now());

                // Asked for once, it is asked for again by the person
                // once the reason is gone; on its own it is tried again.
                let _ = self.store.update(&job.id, |job| {
                    job.merge_requested = false;
                    job.error = Some(format!("not merged yet: {reason}"));
                });

                return false;
            }
        }

        true
    }

    async fn merged(&mut self, job: &Job, commit: Option<String>) {
        self.merge_tried.remove(&job.id);

        let mut note = None;

        if let Some(worktree) = &job.worktree
            && let Err(error) =
                merge::remove_worktree(self.store.root(), worktree, job.branch.as_deref(), false)
                    .await
        {
            note = Some(format!("merged, but its worktree was not removed: {error}"));
        }

        let _ = self.store.update(&job.id, |job| {
            job.status = JobStatus::Merged;
            job.merge_requested = false;
            job.merged_commit = commit;
            job.conflicts.clear();
            job.overlaps.clear();
            job.error = note;

            if job.error.is_none() {
                job.worktree = None;
            }
        });
    }

    /// Starts queued jobs that may start, up to the number allowed.
    /// Returns whether any queued job is still to start.
    async fn start_ready(&mut self, all: &[Job]) -> bool {
        let mut waiting = false;

        for job in all.iter().filter(|job| job.status == JobStatus::Queued) {
            if self.active.contains_key(&job.id) || job.cancel_requested {
                continue;
            }

            let mode = match job.resolve_requested {
                true => Mode::Resolve,
                false => match JobStore::readiness(job, all) {
                    Readiness::Ready => Mode::Work,
                    Readiness::Waiting(_) | Readiness::Blocked(_) => continue,
                },
            };

            // Without worktrees, jobs take turns in the project itself.
            let room = match self.git {
                true => self.active.len() < self.parallel,
                false => self.active.is_empty(),
            };

            if !room {
                waiting = true;

                continue;
            }

            if let Err(error) = self.start(job, mode).await {
                let now = jobs::now_ms();

                let _ = self.store.update(&job.id, |job| {
                    job.status = JobStatus::Failed;
                    job.finished_ms = Some(now);
                    job.error = Some(error);
                });
            }
        }

        waiting
    }

    async fn start(&mut self, job: &Job, mode: Mode) -> Result<(), String> {
        let root = self.store.root().to_path_buf();

        let (worktree, branch, base) = match (self.git, &job.worktree) {
            (false, _) => (None, None, None),

            (true, Some(path)) if path.exists() => {
                (Some(path.clone()), job.branch.clone(), job.base.clone())
            }

            (true, Some(path)) => {
                return Err(format!(
                    "its worktree {} is gone; remove the job and queue it again",
                    path.display()
                ));
            }

            (true, None) => {
                let made = worktree::create(&root, &job.id).await?;
                let base = merge::head(&made.path).await?;

                (Some(made.path), Some(made.branch), Some(base))
            }
        };

        let top = worktree.clone().unwrap_or(root);
        let place = top.join(&job.folder);

        std::fs::create_dir_all(&place)
            .map_err(|error| format!("cannot create {}: {error}", place.display()))?;

        let instruction = match mode {
            Mode::Work => brief(job, branch.as_deref()),

            Mode::Resolve => {
                let worktree = worktree
                    .as_ref()
                    .ok_or("a job in the project itself has nothing to resolve")?;

                let files = match merge::in_progress(worktree).await {
                    // Taken up again after a runner stopped mid-way.
                    true => merge::unresolved(worktree).await,

                    false => {
                        let target = merge::head(self.store.root()).await?;

                        merge::merge_into_worktree(worktree, &target).await?
                    }
                };

                resolve_brief(job, branch.as_deref().unwrap_or(""), &files)
            }
        };

        let now = jobs::now_ms();

        self.store.update(&job.id, |job| {
            job.status = JobStatus::Running;
            job.worktree = worktree.clone();
            job.branch = branch.clone();
            job.base = base.clone();
            job.started_ms.get_or_insert(now);
            job.finished_ms = None;
            job.heartbeat_ms = Some(now);
            job.attempts += 1;
            job.error = None;
        })?;

        let inbox = Arc::new(Inbox::new());
        let counts = Arc::new(Mutex::new(Counts {
            steps: job.steps,
            tokens: job.tokens,
        }));

        let mut model = self.model.clone();

        if let Some(steps) = job.max_steps {
            model.max_steps = steps;
        }

        let task = tokio::spawn(carry_out(Work {
            place,
            model,
            instruction,
            log: self.store.log_path(&job.id),
            state: self.store.state_path(&job.id),
            inbox: inbox.clone(),
            counts: counts.clone(),
        }));

        eprintln!("Started {}: {}", job.id, job.title);

        self.active.insert(
            job.id.clone(),
            Active {
                task,
                inbox,
                mode,
                counts,
            },
        );

        Ok(())
    }
}

/// What one agent run of a job needs.
struct Work {
    place: PathBuf,
    model: ModelArgs,
    instruction: String,
    log: PathBuf,
    state: PathBuf,
    inbox: Arc<Inbox>,
    counts: Arc<Mutex<Counts>>,
}

/// Runs the job's agent to the end: from its task record when that
/// holds a run of the same instruction that was cut off, afresh
/// otherwise.
async fn carry_out(work: Work) -> Result<Report, String> {
    let args = SessionArgs {
        dir: work.place.clone(),
        yes: false,
        json: false,
        model: work.model,
    };

    let options = BuildOptions {
        approver: Some(Arc::new(JobApprover)),
        log: Some(work.log),
        append_log: true,
        quiet: true,
    };

    let agent = build_agent_with(&args, options)
        .await
        .map_err(|error| format!("the agent could not be set up: {error:#}"))?;

    let counts: Arc<dyn TrajectorySink> = Arc::new(CountSink(work.counts));

    let prior = TaskState::load(&work.state)
        .filter(|state| state.can_resume() && state.instruction == work.instruction);

    let mut conversation;

    let ran = match prior {
        Some(state) => {
            conversation = state.resumable_conversation();

            let progress = state.progress();
            let record = TaskStateSink::resuming(&work.state, state, conversation.clone());

            let agent = agent
                .with_inbox(work.inbox)
                .with_sink(counts)
                .with_sink(Arc::new(record));

            agent.resume(&mut conversation, progress).await
        }

        None => {
            conversation = Vec::new();

            let record = TaskStateSink::new(&work.state, &work.instruction);

            let agent = agent
                .with_inbox(work.inbox)
                .with_sink(counts)
                .with_sink(Arc::new(record));

            agent.run(&mut conversation, &work.instruction).await
        }
    };

    match ran {
        Ok(report) => Ok(Report {
            outcome: report.outcome,
            verified: report.verified,
            steps: report.steps,
            tokens: report.usage.total(),
            summary: report.final_text,
        }),

        Err(AgentError::Provider { error, .. }) => Err(match error.is_retryable() {
            true => format!("the model could not be reached ({error}); retrying carries on"),
            false => format!("the model refused the request: {error}"),
        }),
    }
}

/// Keeps a running job's step and token counts where the runner reads
/// them, without its task record having to be read.
struct CountSink(Arc<Mutex<Counts>>);

impl TrajectorySink for CountSink {
    fn record(&self, event: &Event) {
        let mut counts = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        match event {
            Event::ModelResponded { step, usage, .. } => {
                counts.steps = *step;
                counts.tokens += usage.total();
            }

            Event::RunFinished { steps, usage, .. } => {
                counts.steps = *steps;
                counts.tokens = usage.total();
            }

            _ => {}
        }
    }
}

/// What a job's agent is told about where it is, before its task.
fn brief(job: &Job, branch: Option<&str>) -> String {
    let place = match branch {
        Some(branch) => format!(
            "You work in a git worktree of your own, on branch `{branch}`, started from \
             the project's last commit; changes the person has not committed are not here. \
             Other jobs may be working at the same time in their own worktrees. When you \
             finish, everything you changed is committed and merged back for you after a \
             conflict check, so do not commit, merge, rebase, push or change branches \
             yourself."
        ),

        None => "You work in the project itself; no other job runs while you do.".to_string(),
    };

    format!(
        "You are background job {} (\"{}\"). {place} Nobody can answer questions while \
         you work: make reasonable decisions, and say what you assumed. Notes from the job \
         runner about other jobs may arrive between steps. End with a short report: what \
         you changed, how you checked it, and anything left undone.\n\nTask:\n{}",
        job.id, job.title, job.instruction
    )
}

/// What an agent resolving a job's conflicts is told.
fn resolve_brief(job: &Job, branch: &str, files: &[String]) -> String {
    let files = match files.is_empty() {
        true => "(git lists none; check `git status`)".to_string(),
        false => files.join(", "),
    };

    format!(
        "You are resolving the merge conflicts of background job {} (\"{}\") on branch \
         `{branch}`, in its own worktree. The project's latest commit has been merged into \
         this branch, and these files hold conflict markers: {files}.\n\nFor each file, \
         read the whole conflict and what each side changed (`git diff MERGE_HEAD...HEAD` \
         and `git diff HEAD...MERGE_HEAD`), and write the resolution that keeps the intent \
         of both: the job's own task, and what the project gained meanwhile. Remove every \
         marker, then run the project's build and tests and fix what the merge broke. Do \
         not commit, abort the merge, push or change branches: the job runner concludes \
         the merge and merges the branch. End with each file and how you resolved it.\n\n\
         The job's task was:\n{}",
        job.id, job.title, job.instruction
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_model_arguments_parse_as_cortex_run_takes_them() {
        let model = parse_model_args(&[
            "--provider".to_string(),
            "local".to_string(),
            "--max-steps".to_string(),
            "7".to_string(),
        ])
        .unwrap();

        assert_eq!(model.max_steps, 7);
        assert!(parse_model_args(&["--nonsense".to_string()]).is_err());
        assert!(parse_model_args(&[]).is_ok());
    }

    #[test]
    fn a_job_is_told_where_it_works_and_what_to_leave_alone() {
        let job = Job {
            id: "job-2".to_string(),
            title: "Parser".to_string(),
            instruction: "Write the parser.".to_string(),
            ..serde_json::from_value(serde_json::json!({
                "id": "x", "title": "x", "instruction": "x",
                "status": "queued", "created_ms": 0,
            }))
            .unwrap()
        };

        let text = brief(&job, Some("cortex/job-2"));

        assert!(text.contains("job-2"));
        assert!(text.contains("cortex/job-2"));
        assert!(text.ends_with("Write the parser."));
        assert!(brief(&job, None).contains("project itself"));
        assert!(resolve_brief(&job, "cortex/job-2", &["a.rs".into()]).contains("a.rs"));
    }
}
