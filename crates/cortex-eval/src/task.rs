//! Loading suites and tasks from disk.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

const TASK_FILE: &str = "task.toml";

#[derive(Debug, Clone, Deserialize)]
pub struct CheckSpec {
    /// Shell command run in the finished project. Exit code 0 means solved.
    pub command: String,

    #[serde(default = "default_check_timeout")]
    pub timeout_seconds: u64,
}

fn default_check_timeout() -> u64 {
    120
}

/// One separately checkable part of a long task.
///
/// A long task that is scored only as solved or not gives nothing to
/// compare when no configuration solves it. With milestones, an attempt
/// that fixed three of five problems scores above one that fixed one.
#[derive(Debug, Clone, Deserialize)]
pub struct MilestoneSpec {
    pub name: String,

    /// Shell command run in the finished project, after the hidden files
    /// are in place. Exit code 0 means this part is done.
    pub command: String,

    /// How much this part counts towards the task's score.
    #[serde(default = "default_weight")]
    pub weight: f64,

    #[serde(default = "default_check_timeout")]
    pub timeout_seconds: u64,
}

fn default_weight() -> f64 {
    1.0
}

/// A git repository the task starts from, instead of a `repo/` directory.
///
/// The working copy is built from a clean export of `commit`. Files in the
/// task's own `repo/` directory, if any, are then copied over it; that is
/// how a task plants a bug in a real project.
#[derive(Debug, Clone, Deserialize)]
pub struct SourceSpec {
    /// Path to the repository, relative to the task directory.
    pub repo: PathBuf,

    pub commit: String,

    /// Paths deleted from the export before the agent starts, normally the
    /// tests that will score it.
    #[serde(default)]
    pub remove: Vec<String>,

    /// Paths symlinked from the repository into the working copy instead of
    /// being copied, for large installed dependencies such as
    /// `node_modules`.
    #[serde(default)]
    pub link: Vec<String>,

    /// Directory holding the linked paths, relative to the task directory.
    /// Keeping dependencies outside the repository means a link cannot be
    /// followed back to the original source and tests. Defaults to `repo`.
    #[serde(default)]
    pub link_from: Option<PathBuf>,
}

/// The contents of `task.toml`.
#[derive(Debug, Deserialize)]
struct TaskFile {
    instruction: String,
    check: CheckSpec,

    #[serde(default)]
    source: Option<SourceSpec>,

    #[serde(default)]
    max_steps: Option<u32>,

    #[serde(default, rename = "milestone")]
    milestones: Vec<MilestoneSpec>,
}

#[derive(Debug, Clone)]
pub struct Task {
    /// The task directory's name.
    pub id: String,
    pub dir: PathBuf,
    pub instruction: String,
    pub check: CheckSpec,
    pub source: Option<SourceSpec>,
    pub max_steps: Option<u32>,

    /// The parts of the task that are scored one by one. Empty for a task
    /// that is only solved or not.
    pub milestones: Vec<MilestoneSpec>,
}

impl Task {
    pub fn repo_dir(&self) -> PathBuf {
        self.dir.join("repo")
    }

    pub fn hidden_dir(&self) -> PathBuf {
        self.dir.join("hidden")
    }

    pub fn solution_dir(&self) -> PathBuf {
        self.dir.join("solution")
    }

    /// Absolute path of the source repository, when the task has one.
    pub fn source_repo(&self) -> Option<PathBuf> {
        self.source
            .as_ref()
            .map(|source| self.dir.join(&source.repo))
    }

    /// Absolute path of the directory that linked paths come from.
    pub fn link_dir(&self) -> Option<PathBuf> {
        let source = self.source.as_ref()?;

        let relative = source.link_from.as_ref().unwrap_or(&source.repo);

        Some(self.dir.join(relative))
    }

    fn load(dir: &Path) -> Result<Self> {
        let file = dir.join(TASK_FILE);

        let text = fs::read_to_string(&file)
            .with_context(|| format!("could not read {}", file.display()))?;

        let parsed: TaskFile =
            toml::from_str(&text).with_context(|| format!("could not parse {}", file.display()))?;

        let id = dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();

        let task = Self {
            id,
            dir: dir.to_path_buf(),
            instruction: parsed.instruction.trim().to_string(),
            check: parsed.check,
            source: parsed.source,
            max_steps: parsed.max_steps,
            milestones: parsed.milestones,
        };

        if task.source.is_none() && !task.repo_dir().is_dir() {
            bail!(
                "task `{}` has neither a repo/ directory nor a [source]",
                task.id
            );
        }

        Ok(task)
    }
}

#[derive(Debug, Clone)]
pub struct Suite {
    pub name: String,
    pub tasks: Vec<Task>,
}

impl Suite {
    /// Loads every subdirectory that contains a `task.toml`, sorted by name.
    pub fn load(dir: &Path) -> Result<Self> {
        let entries = fs::read_dir(dir)
            .with_context(|| format!("could not read the suite directory {}", dir.display()))?;

        let mut task_dirs: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.join(TASK_FILE).is_file())
            .collect();

        task_dirs.sort();

        let tasks = task_dirs
            .iter()
            .map(|path| Task::load(path))
            .collect::<Result<Vec<_>>>()?;

        if tasks.is_empty() {
            bail!("no tasks found in {}", dir.display());
        }

        let name = dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "suite".to_string());

        Ok(Self { name, tasks })
    }

    /// Keeps only tasks whose id contains `filter`.
    pub fn filtered(mut self, filter: Option<&str>) -> Self {
        if let Some(filter) = filter {
            self.tasks.retain(|task| task.id.contains(filter));
        }

        self
    }
}
