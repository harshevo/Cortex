//! Background jobs: tasks that run on their own, beside the conversation
//! and after the window that started them is closed.
//!
//! A job is a whole instruction for an agent of its own. Jobs are kept on
//! disk in `.cortex/jobs` of the project they belong to, one folder each,
//! so they outlive the process that queued them and can be watched from
//! any window. A job runner, a process of its own (`cortex jobs serve`),
//! takes queued jobs whose dependencies are met and runs several at once.
//! In a git repository each job works in a worktree on a branch of its
//! own, so jobs running together never write over each other; when one
//! finishes, its branch is merged back one job at a time, after checking
//! that the merge is clean (see [`crate::merge`]).
//!
//! Every process may read the jobs. Changes are made under a lock on the
//! folder that works across processes, so the runner, the window and the
//! command line never lose each other's changes.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::task_state::write_atomically;
use crate::trajectory::unix_millis;

/// Where the jobs are kept, relative to the project.
pub const FOLDER: &str = ".cortex/jobs";

/// How deep jobs may start jobs of their own: a job started by a person
/// or a chat is at depth 0.
pub const MAX_DEPTH: u32 = 2;

/// Jobs that may wait or run at once, so that a model that keeps
/// starting jobs cannot fill the machine.
pub const MAX_OPEN: usize = 32;

/// The longest instruction a job takes.
pub const MAX_INSTRUCTION_BYTES: usize = 32_768;

/// How often a runner says it is alive.
pub const HEARTBEAT: Duration = Duration::from_secs(2);

/// A runner silent for this long is taken to have died.
pub const STALE_AFTER: Duration = Duration::from_secs(15);

/// Jobs a runner carries out at once unless told otherwise.
pub const DEFAULT_PARALLEL: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    /// Waiting for a runner, or for the jobs it depends on.
    Queued,

    Running,

    /// The agent finished. In a repository its work waits on its branch
    /// to be merged.
    Done,

    /// Its branch was merged into the project.
    Merged,

    /// Its branch cannot be merged without resolving conflicts.
    Conflicted,

    Failed,

    Cancelled,
}

impl JobStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Done => "done",
            Self::Merged => "merged",
            Self::Conflicted => "conflicted",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    /// Whether nothing more will happen to the job unless asked.
    pub fn is_settled(self) -> bool {
        matches!(self, Self::Merged | Self::Failed | Self::Cancelled)
    }

    /// Queued or running.
    pub fn is_open(self) -> bool {
        matches!(self, Self::Queued | Self::Running)
    }
}

/// Files another job is changing too.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Overlap {
    pub job: String,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub title: String,
    pub instruction: String,

    /// Jobs that must be finished, and in a repository merged, first.
    #[serde(default)]
    pub depends_on: Vec<String>,

    pub status: JobStatus,

    /// Who queued it: `person`, a chat's agent id, or `job-N`.
    #[serde(default)]
    pub created_by: String,

    /// How many jobs above it started it.
    #[serde(default)]
    pub depth: u32,

    /// Merge the branch as soon as the job finishes well. Without it the
    /// branch waits for [`JobStore::request_merge`].
    #[serde(default = "yes")]
    pub auto_merge: bool,

    /// Model calls the job may make, instead of the runner's setting.
    #[serde(default)]
    pub max_steps: Option<u32>,

    /// Where the agent works and on which branch; `None` when the project
    /// is not a repository and the job works in the project itself.
    #[serde(default)]
    pub worktree: Option<PathBuf>,
    #[serde(default)]
    pub branch: Option<String>,

    /// The commit the job's branch started from.
    #[serde(default)]
    pub base: Option<String>,

    pub created_ms: u64,
    #[serde(default)]
    pub started_ms: Option<u64>,
    #[serde(default)]
    pub finished_ms: Option<u64>,

    /// Last sign of life from the runner carrying the job out.
    #[serde(default)]
    pub heartbeat_ms: Option<u64>,

    #[serde(default)]
    pub steps: u32,
    #[serde(default)]
    pub tokens: u64,

    /// The agent's outcome, as its log names it.
    #[serde(default)]
    pub outcome: Option<String>,
    #[serde(default)]
    pub verified: Option<bool>,

    /// The agent's last words: what it did.
    #[serde(default)]
    pub summary: String,

    /// What went wrong, or why a merge is waiting.
    #[serde(default)]
    pub error: Option<String>,

    /// Files the job has changed so far, relative to the project.
    #[serde(default)]
    pub changed_files: Vec<String>,

    /// Other jobs changing the same files.
    #[serde(default)]
    pub overlaps: Vec<Overlap>,

    /// Files that conflict with the project when merged.
    #[serde(default)]
    pub conflicts: Vec<String>,

    #[serde(default)]
    pub merged_commit: Option<String>,

    #[serde(default)]
    pub cancel_requested: bool,
    #[serde(default)]
    pub merge_requested: bool,

    /// Run an agent in the job's worktree to resolve its conflicts.
    #[serde(default)]
    pub resolve_requested: bool,

    /// Times it was taken up again after its runner stopped.
    #[serde(default)]
    pub resumed: u32,

    /// Times it was started, retries included.
    #[serde(default)]
    pub attempts: u32,
}

fn yes() -> bool {
    true
}

impl Job {
    /// Its number, for ordering: `job-12` is 12.
    pub fn number(&self) -> u64 {
        number_of(&self.id).unwrap_or(u64::MAX)
    }

    /// Time spent so far, or in all once finished.
    pub fn elapsed(&self, now_ms: u64) -> Option<Duration> {
        let started = self.started_ms?;
        let ended = self.finished_ms.unwrap_or(now_ms);

        Some(Duration::from_millis(ended.saturating_sub(started)))
    }

    /// Whether a job that depends on this one may start.
    pub fn lets_dependents_start(&self) -> bool {
        match self.status {
            JobStatus::Merged => true,

            // Work done in the project itself is already there.
            JobStatus::Done => self.branch.is_none(),

            _ => false,
        }
    }

    /// Whether a job that depends on this one can never start unless
    /// this one is retried.
    pub fn stops_dependents(&self) -> bool {
        matches!(self.status, JobStatus::Failed | JobStatus::Cancelled)
    }
}

/// What a queued job is waiting for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Readiness {
    Ready,

    /// These jobs are not finished, or not merged.
    Waiting(Vec<String>),

    /// This job failed, was cancelled or is gone.
    Blocked(String),
}

/// A job to be queued.
#[derive(Debug, Clone, Default)]
pub struct NewJob {
    pub title: String,
    pub instruction: String,
    pub depends_on: Vec<String>,
    pub created_by: String,
    pub depth: u32,
    pub auto_merge: bool,
    pub max_steps: Option<u32>,
}

/// The jobs of one project.
#[derive(Debug, Clone)]
pub struct JobStore {
    root: PathBuf,
    dir: PathBuf,
}

const JOB_FILE: &str = "job.json";
const LOG_FILE: &str = "log.jsonl";
const STATE_FILE: &str = "state.json";
const RUNNER_FILE: &str = "runner.json";
const CONFIG_FILE: &str = "runner-config.json";
const LOCK_FILE: &str = ".lock";

/// A lock older than this was left by a process that died holding it.
const LOCK_STALE_AFTER: Duration = Duration::from_secs(10);

impl JobStore {
    /// The jobs of the project at `root`. A worktree's jobs are the main
    /// checkout's, so a job that starts jobs adds them to the same list.
    pub fn for_project(root: impl AsRef<Path>) -> Self {
        let root = project_root(root.as_ref());
        let dir = root.join(FOLDER);

        Self { root, dir }
    }

    /// The main checkout of the project.
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn job_dir(&self, id: &str) -> PathBuf {
        self.dir.join(id)
    }

    /// The job's full log, in the same form as a session's.
    pub fn log_path(&self, id: &str) -> PathBuf {
        self.job_dir(id).join(LOG_FILE)
    }

    /// The job's [`crate::task_state::TaskState`].
    pub fn state_path(&self, id: &str) -> PathBuf {
        self.job_dir(id).join(STATE_FILE)
    }

    /// Every job, oldest first.
    pub fn list(&self) -> Vec<Job> {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return Vec::new();
        };

        let mut jobs: Vec<Job> = entries
            .filter_map(Result::ok)
            .filter(|entry| entry.path().is_dir())
            .filter_map(|entry| read_job(&entry.path().join(JOB_FILE)))
            .collect();

        jobs.sort_by_key(|job| (job.number(), job.id.clone()));

        jobs
    }

    pub fn get(&self, id: &str) -> Option<Job> {
        if !valid_id(id) {
            return None;
        }

        read_job(&self.job_dir(id).join(JOB_FILE))
    }

    /// Queues a job. Its dependencies must be jobs that exist.
    pub fn add(&self, new: NewJob) -> Result<Job, String> {
        let instruction = new.instruction.trim();

        if instruction.is_empty() {
            return Err("a job needs an instruction".to_string());
        }

        if instruction.len() > MAX_INSTRUCTION_BYTES {
            return Err(format!(
                "the instruction is longer than {MAX_INSTRUCTION_BYTES} bytes"
            ));
        }

        if new.depth > MAX_DEPTH {
            return Err(format!(
                "jobs may start jobs only {MAX_DEPTH} levels deep; do this part yourself"
            ));
        }

        let _lock = self.lock()?;

        let jobs = self.list();

        let open = jobs.iter().filter(|job| job.status.is_open()).count();

        if open >= MAX_OPEN {
            return Err(format!(
                "{open} jobs are already queued or running; wait for some to finish"
            ));
        }

        let known: BTreeSet<&str> = jobs.iter().map(|job| job.id.as_str()).collect();

        let mut depends_on = Vec::new();

        for dependency in &new.depends_on {
            let dependency = normalize_id(dependency);

            if !known.contains(dependency.as_str()) {
                return Err(format!("there is no job `{dependency}` to depend on"));
            }

            if !depends_on.contains(&dependency) {
                depends_on.push(dependency);
            }
        }

        let next = jobs
            .iter()
            .filter_map(|job| number_of(&job.id))
            .max()
            .unwrap_or(0)
            + 1;

        let title = match new.title.trim() {
            "" => crate_title(instruction),
            title => title.chars().take(80).collect(),
        };

        let job = Job {
            id: format!("job-{next}"),
            title,
            instruction: instruction.to_string(),
            depends_on,
            status: JobStatus::Queued,
            created_by: new.created_by,
            depth: new.depth,
            auto_merge: new.auto_merge,
            max_steps: new.max_steps,
            worktree: None,
            branch: None,
            base: None,
            created_ms: now_ms(),
            started_ms: None,
            finished_ms: None,
            heartbeat_ms: None,
            steps: 0,
            tokens: 0,
            outcome: None,
            verified: None,
            summary: String::new(),
            error: None,
            changed_files: Vec::new(),
            overlaps: Vec::new(),
            conflicts: Vec::new(),
            merged_commit: None,
            cancel_requested: false,
            merge_requested: false,
            resolve_requested: false,
            resumed: 0,
            attempts: 0,
        };

        self.write(&job).map_err(|error| error.to_string())?;

        Ok(job)
    }

    /// Changes the job `id` with `change`, under the lock, and returns it
    /// as it was written.
    pub fn update(&self, id: &str, change: impl FnOnce(&mut Job)) -> Result<Job, String> {
        let id = normalize_id(id);
        let _lock = self.lock()?;

        let mut job = self
            .get(&id)
            .ok_or_else(|| format!("there is no job `{id}`"))?;

        change(&mut job);

        self.write(&job).map_err(|error| error.to_string())?;

        Ok(job)
    }

    /// Asks for the job to be stopped. A queued job is cancelled at once;
    /// a running one by its runner, within a heartbeat.
    pub fn request_cancel(&self, id: &str) -> Result<Job, String> {
        let current = self.require(id)?;

        if !current.status.is_open() {
            return Err(format!(
                "{} is {}; only a queued or running job can be stopped",
                current.id,
                current.status.label()
            ));
        }

        self.update(id, |job| match job.status {
            JobStatus::Queued => {
                job.status = JobStatus::Cancelled;
                job.finished_ms = Some(now_ms());
            }

            _ => job.cancel_requested = true,
        })
    }

    /// Asks for the job's branch to be merged, by the runner, which
    /// merges one job at a time.
    pub fn request_merge(&self, id: &str) -> Result<Job, String> {
        let current = self.require(id)?;

        if current.branch.is_none() {
            return Err(format!(
                "{} worked in the project itself; there is nothing to merge",
                current.id
            ));
        }

        if current.status != JobStatus::Done {
            return Err(format!(
                "{} is {}; only a finished job can be merged",
                current.id,
                current.status.label()
            ));
        }

        self.update(id, |job| {
            job.merge_requested = true;
            job.error = None;
        })
    }

    /// Asks for an agent to resolve the conflicts of the job's branch, in
    /// its worktree, after which it is merged as usual.
    pub fn request_resolve(&self, id: &str) -> Result<Job, String> {
        let current = self.require(id)?;

        if current.status != JobStatus::Conflicted {
            return Err(format!("{} has no conflicts to resolve", current.id));
        }

        self.update(id, |job| {
            job.resolve_requested = true;
            job.status = JobStatus::Queued;
            job.error = None;
        })
    }

    /// Queues a failed or cancelled job again. It carries on from where
    /// it stopped when its record allows.
    pub fn retry(&self, id: &str) -> Result<Job, String> {
        let current = self.require(id)?;

        if !matches!(current.status, JobStatus::Failed | JobStatus::Cancelled) {
            return Err(format!(
                "{} is {}; only a failed or cancelled job can be retried",
                current.id,
                current.status.label()
            ));
        }

        self.update(id, |job| {
            job.status = JobStatus::Queued;
            job.error = None;
            job.cancel_requested = false;
            job.finished_ms = None;
        })
    }

    /// Removes a job that is not queued or running, and its folder. Its
    /// worktree and branch are left to the caller.
    pub fn remove(&self, id: &str) -> Result<Job, String> {
        let id = normalize_id(id);
        let _lock = self.lock()?;

        let job = self
            .get(&id)
            .ok_or_else(|| format!("there is no job `{id}`"))?;

        if job.status.is_open() {
            return Err(format!("{} is {}; stop it first", job.id, job.status.label()));
        }

        let dependents: Vec<String> = self
            .list()
            .into_iter()
            .filter(|other| other.status.is_open() && other.depends_on.contains(&job.id))
            .map(|other| other.id)
            .collect();

        if !dependents.is_empty() {
            return Err(format!(
                "{} waits on {}; remove or cancel it first",
                dependents.join(", "),
                job.id
            ));
        }

        fs::remove_dir_all(self.job_dir(&job.id)).map_err(|error| error.to_string())?;

        Ok(job)
    }

    /// What the job `job` waits for, given every job.
    pub fn readiness(job: &Job, jobs: &[Job]) -> Readiness {
        let by_id: BTreeMap<&str, &Job> = jobs.iter().map(|job| (job.id.as_str(), job)).collect();

        let mut waiting = Vec::new();

        for dependency in &job.depends_on {
            match by_id.get(dependency.as_str()) {
                None => return Readiness::Blocked(dependency.clone()),

                Some(other) if other.stops_dependents() => {
                    return Readiness::Blocked(dependency.clone());
                }

                Some(other) if !other.lets_dependents_start() => waiting.push(dependency.clone()),

                Some(_) => {}
            }
        }

        match waiting.is_empty() {
            true => Readiness::Ready,
            false => Readiness::Waiting(waiting),
        }
    }

    fn require(&self, id: &str) -> Result<Job, String> {
        let id = normalize_id(id);

        self.get(&id).ok_or_else(|| format!("there is no job `{id}`"))
    }

    fn write(&self, job: &Job) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(job)?;

        write_atomically(&self.job_dir(&job.id).join(JOB_FILE), &bytes)
    }

    /// Holds the folder's lock until dropped. Another process holding it
    /// is waited for; a lock left by one that died is taken over.
    pub fn lock(&self) -> Result<StoreLock, String> {
        fs::create_dir_all(&self.dir).map_err(|error| {
            format!("cannot create {}: {error}", self.dir.display())
        })?;

        let path = self.dir.join(LOCK_FILE);
        let started = Instant::now();

        loop {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    let _ = write!(file, "{}", std::process::id());

                    return Ok(StoreLock { path });
                }

                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    let stale = fs::metadata(&path)
                        .and_then(|metadata| metadata.modified())
                        .ok()
                        .and_then(|modified| modified.elapsed().ok())
                        .is_some_and(|age| age > LOCK_STALE_AFTER);

                    if stale {
                        let _ = fs::remove_file(&path);

                        continue;
                    }

                    if started.elapsed() > LOCK_STALE_AFTER * 2 {
                        return Err("the jobs folder stayed locked".to_string());
                    }

                    std::thread::sleep(Duration::from_millis(5));
                }

                Err(error) => return Err(format!("cannot lock {}: {error}", path.display())),
            }
        }
    }

    /// The runner's last report, if one has run here.
    pub fn runner(&self) -> Option<RunnerRecord> {
        let text = fs::read_to_string(self.dir.join(RUNNER_FILE)).ok()?;

        serde_json::from_str(&text).ok()
    }

    /// Whether a runner is carrying out this project's jobs now.
    pub fn runner_alive(&self) -> bool {
        self.runner().is_some_and(|runner| runner.is_alive(now_ms()))
    }

    /// Claims the project for the runner with this process id: `false`
    /// when another live runner has it.
    pub fn claim_runner(&self, pid: u32) -> Result<bool, String> {
        let _lock = self.lock()?;

        if let Some(runner) = self.runner()
            && runner.pid != pid
            && runner.is_alive(now_ms())
        {
            return Ok(false);
        }

        self.write_runner(pid, false)?;

        Ok(true)
    }

    /// Says the runner `pid` is alive.
    pub fn beat(&self, pid: u32) -> Result<(), String> {
        let _lock = self.lock()?;

        // A runner that lost its claim to another stops writing.
        if let Some(runner) = self.runner()
            && runner.pid != pid
            && runner.is_alive(now_ms())
        {
            return Err("another runner has taken over".to_string());
        }

        self.write_runner(pid, false)
    }

    /// Gives up the claim of runner `pid`.
    pub fn release_runner(&self, pid: u32) {
        let Ok(_lock) = self.lock() else {
            return;
        };

        if self.runner().is_some_and(|runner| runner.pid == pid) {
            let _ = fs::remove_file(self.dir.join(RUNNER_FILE));
        }
    }

    fn write_runner(&self, pid: u32, starting: bool) -> Result<(), String> {
        let record = RunnerRecord {
            pid,
            heartbeat_ms: now_ms(),
            starting,
        };

        let bytes = serde_json::to_vec(&record).map_err(|error| error.to_string())?;

        write_atomically(&self.dir.join(RUNNER_FILE), &bytes).map_err(|error| error.to_string())
    }

    /// How a runner for this project is set up: the model arguments it
    /// was last started with.
    pub fn config(&self) -> Option<RunnerConfig> {
        let text = fs::read_to_string(self.dir.join(CONFIG_FILE)).ok()?;

        serde_json::from_str(&text).ok()
    }

    pub fn save_config(&self, config: &RunnerConfig) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(config).map_err(|error| error.to_string())?;

        write_atomically(&self.dir.join(CONFIG_FILE), &bytes).map_err(|error| error.to_string())
    }

    /// Makes sure a runner is carrying out this project's jobs, starting
    /// one if none is. Several calls at once start one runner: the start
    /// is noted under the lock, and a runner that finds another already
    /// running leaves at once.
    pub fn ensure_runner(&self) -> RunnerStart {
        let Ok(lock) = self.lock() else {
            return RunnerStart::Failed("the jobs folder is locked".to_string());
        };

        if let Some(runner) = self.runner()
            && runner.is_alive(now_ms())
        {
            return RunnerStart::Running;
        }

        // Noted before it starts, so that another call does not start a
        // second one meanwhile.
        if let Err(error) = self.write_runner(0, true) {
            return RunnerStart::Failed(error);
        }

        drop(lock);

        let launched = match launcher() {
            Some(launch) => launch(&self.root),
            None => default_launch(self),
        };

        match launched {
            Ok(()) => RunnerStart::Started,

            Err(error) => {
                let _ = fs::remove_file(self.dir.join(RUNNER_FILE));

                RunnerStart::Failed(error)
            }
        }
    }
}

/// Removes the folder's lock file when dropped.
pub struct StoreLock {
    path: PathBuf,
}

impl Drop for StoreLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerRecord {
    /// 0 while one is being started.
    pub pid: u32,
    pub heartbeat_ms: u64,

    #[serde(default)]
    pub starting: bool,
}

impl RunnerRecord {
    pub fn is_alive(&self, now_ms: u64) -> bool {
        now_ms.saturating_sub(self.heartbeat_ms) < STALE_AFTER.as_millis() as u64
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerConfig {
    /// The `cortex` command's model arguments, such as
    /// `["--provider", "anthropic", "--model", "claude-opus-5-5"]`. Keys
    /// are never kept here; they come from the environment or keychain.
    pub model_args: Vec<String>,

    #[serde(default = "default_parallel")]
    pub max_parallel: usize,
}

fn default_parallel() -> usize {
    DEFAULT_PARALLEL
}

/// What came of [`JobStore::ensure_runner`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunnerStart {
    Running,
    Started,
    Failed(String),
}

/// Starts a runner for the project at the path given.
pub type Launcher = dyn Fn(&Path) -> Result<(), String> + Send + Sync;

fn launcher_slot() -> &'static RwLock<Option<Arc<Launcher>>> {
    static SLOT: OnceLock<RwLock<Option<Arc<Launcher>>>> = OnceLock::new();

    SLOT.get_or_init(|| RwLock::new(None))
}

/// Sets how this process starts runners, in place of the default: the
/// desktop app passes the model's key, which a runner started as a plain
/// command would not have.
pub fn set_launcher(launch: Arc<Launcher>) {
    *launcher_slot()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(launch);
}

fn launcher() -> Option<Arc<Launcher>> {
    launcher_slot()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// Starts this program again as `<program> jobs serve --dir <root>`,
/// which takes the model arguments the project's runner was last set up
/// with. Both `cortex` and the desktop app answer to that.
fn default_launch(store: &JobStore) -> Result<(), String> {
    if store.config().is_none() {
        return Err(
            "no job runner has been set up for this project; start one with \
             `cortex jobs serve` and the model arguments to use"
                .to_string(),
        );
    }

    let program = std::env::current_exe().map_err(|error| error.to_string())?;

    let root = store.root.to_string_lossy().into_owned();

    spawn_detached(
        &program,
        &["jobs", "serve", "--dir", &root],
        &[],
        &store.dir.join("runner.log"),
    )
}

/// Starts `program` so that it lives on after this process: its own
/// process group, no terminal, and its output in `log`.
pub fn spawn_detached(
    program: &Path,
    args: &[&str],
    env: &[(String, String)],
    log: &Path,
) -> Result<(), String> {
    if let Some(folder) = log.parent() {
        fs::create_dir_all(folder).map_err(|error| error.to_string())?;
    }

    let output = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .map_err(|error| format!("cannot open {}: {error}", log.display()))?;

    let errors = output.try_clone().map_err(|error| error.to_string())?;

    let mut command = Command::new(program);

    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(output)
        .stderr(errors);

    for (name, value) in env {
        command.env(name, value);
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        // Out of the window's process group, so closing it, or a ^C in
        // the terminal it was started from, does not stop the jobs.
        command.process_group(0);
    }

    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("cannot start {}: {error}", program.display()))
}

/// The main checkout of the repository `path` is in, or `path` itself
/// when it is not in one.
pub fn project_root(path: &Path) -> PathBuf {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

    let common = Command::new("git")
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .current_dir(&path)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| PathBuf::from(String::from_utf8_lossy(&output.stdout).trim()));

    match common {
        // A bare repository has no checkout of its own.
        Some(common) if common.file_name().is_some_and(|name| name == ".git") => common
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or(path),

        _ => path,
    }
}

/// `3`, `#3`, `job 3` and `job-3` all name `job-3`.
pub fn normalize_id(id: &str) -> String {
    let id = id.trim();

    let digits = id
        .trim_start_matches("job")
        .trim_start_matches(['-', ' ', '_', '#']);

    match digits.parse::<u64>() {
        Ok(number) => format!("job-{number}"),
        Err(_) => id.to_string(),
    }
}

fn number_of(id: &str) -> Option<u64> {
    id.strip_prefix("job-")?.parse().ok()
}

fn valid_id(id: &str) -> bool {
    number_of(id).is_some()
}

fn read_job(path: &Path) -> Option<Job> {
    let text = fs::read_to_string(path).ok()?;

    serde_json::from_str(&text).ok()
}

/// A title from the start of an instruction.
fn crate_title(instruction: &str) -> String {
    let line = instruction.lines().next().unwrap_or("").trim();

    match line.chars().count() > 60 {
        true => format!("{}…", line.chars().take(59).collect::<String>().trim_end()),
        false => line.to_string(),
    }
}

pub fn now_ms() -> u64 {
    unix_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new(instruction: &str, depends_on: &[&str]) -> NewJob {
        NewJob {
            instruction: instruction.to_string(),
            depends_on: depends_on.iter().map(|id| id.to_string()).collect(),
            created_by: "person".to_string(),
            auto_merge: true,
            ..NewJob::default()
        }
    }

    #[test]
    fn jobs_are_numbered_kept_and_listed_in_order() {
        let folder = tempfile::tempdir().unwrap();
        let store = JobStore::for_project(folder.path());

        let first = store.add(new("Write the parser", &[])).unwrap();
        let second = store.add(new("Test the parser", &["1"])).unwrap();

        assert_eq!(first.id, "job-1");
        assert_eq!(second.id, "job-2");
        assert_eq!(second.depends_on, vec!["job-1"]);
        assert_eq!(first.title, "Write the parser");

        let ids: Vec<String> = store.list().into_iter().map(|job| job.id).collect();

        assert_eq!(ids, vec!["job-1", "job-2"]);
        assert!(store.add(new("x", &["job-9"])).is_err());
        assert!(store.add(new("  ", &[])).is_err());
        assert!(
            store
                .add(NewJob {
                    depth: MAX_DEPTH + 1,
                    ..new("deep", &[])
                })
                .is_err()
        );
    }

    #[test]
    fn a_dependent_waits_for_a_merge_and_is_blocked_by_a_failure() {
        let folder = tempfile::tempdir().unwrap();
        let store = JobStore::for_project(folder.path());

        store.add(new("a", &[])).unwrap();
        store.add(new("b", &["job-1"])).unwrap();

        let waiting = |store: &JobStore| {
            let jobs = store.list();

            JobStore::readiness(&jobs[1], &jobs)
        };

        assert_eq!(waiting(&store), Readiness::Waiting(vec!["job-1".into()]));

        store
            .update("job-1", |job| {
                job.status = JobStatus::Done;
                job.branch = Some("cortex/job-1".to_string());
            })
            .unwrap();

        // Done on a branch is not yet in the project.
        assert_eq!(waiting(&store), Readiness::Waiting(vec!["job-1".into()]));

        store
            .update("job-1", |job| job.status = JobStatus::Merged)
            .unwrap();

        assert_eq!(waiting(&store), Readiness::Ready);

        store
            .update("job-1", |job| job.status = JobStatus::Failed)
            .unwrap();

        assert_eq!(waiting(&store), Readiness::Blocked("job-1".into()));
    }

    #[test]
    fn requests_follow_the_status() {
        let folder = tempfile::tempdir().unwrap();
        let store = JobStore::for_project(folder.path());

        store.add(new("a", &[])).unwrap();
        store.add(new("b", &["1"])).unwrap();

        assert!(store.request_merge("1").is_err());
        assert!(store.remove("1").is_err());

        let cancelled = store.request_cancel("1").unwrap();

        assert_eq!(cancelled.status, JobStatus::Cancelled);
        assert!(store.request_cancel("1").is_err());

        // Job 2 still waits on it.
        assert!(store.remove("1").is_err());

        store
            .update("job-2", |job| job.status = JobStatus::Running)
            .unwrap();

        let stopping = store.request_cancel("job-2").unwrap();

        assert_eq!(stopping.status, JobStatus::Running);
        assert!(stopping.cancel_requested);

        let retried = store.retry("1").unwrap();

        assert_eq!(retried.status, JobStatus::Queued);
    }

    #[test]
    fn one_runner_holds_a_project_until_it_goes_quiet() {
        let folder = tempfile::tempdir().unwrap();
        let store = JobStore::for_project(folder.path());

        assert!(!store.runner_alive());
        assert!(store.claim_runner(100).unwrap());
        assert!(store.runner_alive());
        assert!(!store.claim_runner(200).unwrap());
        assert!(store.beat(200).is_err());
        assert!(store.beat(100).is_ok());

        store.release_runner(200);
        assert!(store.runner_alive());

        store.release_runner(100);
        assert!(!store.runner_alive());
        assert!(store.claim_runner(200).unwrap());
    }

    #[test]
    fn ids_are_read_loosely_and_paths_stay_inside() {
        assert_eq!(normalize_id("3"), "job-3");
        assert_eq!(normalize_id("#3"), "job-3");
        assert_eq!(normalize_id("job 3"), "job-3");
        assert_eq!(normalize_id("job-3"), "job-3");
        assert_eq!(normalize_id("../x"), "../x");

        let folder = tempfile::tempdir().unwrap();
        let store = JobStore::for_project(folder.path());

        assert!(store.get("../x").is_none());
    }

    #[test]
    fn the_lock_is_shared_and_a_stale_one_is_taken_over() {
        let folder = tempfile::tempdir().unwrap();
        let store = JobStore::for_project(folder.path());

        let held = store.lock().unwrap();
        let path = store.dir().join(LOCK_FILE);

        assert!(path.exists());

        drop(held);

        assert!(!path.exists());

        fs::write(&path, "1").unwrap();

        let old = std::time::SystemTime::now() - Duration::from_secs(60);
        let file = fs::File::options().write(true).open(&path).unwrap();

        file.set_modified(old).unwrap();

        assert!(store.lock().is_ok());
    }
}
