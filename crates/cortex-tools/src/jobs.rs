//! Tools for starting and watching background jobs: agents that work in
//! parallel, each in a worktree of its own, and whose work is merged back
//! when they finish. See [`cortex_core::jobs`].

use std::path::Path;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use cortex_core::jobs::{self, Job, JobStatus, JobStore, NewJob, Readiness, RunnerStart};
use cortex_core::{Access, Tool, ToolContext, ToolError, ToolSpec};
use serde_json::{Value, json};

/// The longest a call to `jobs` may wait for jobs to finish.
const MAX_WAIT: Duration = Duration::from_secs(900);

const POLL: Duration = Duration::from_secs(2);

/// Lines of activity shown for one job.
const ACTIVITY_LINES: usize = 8;

/// `spawn_job`: queues a background job.
pub struct SpawnJob;

/// `jobs`: lists jobs, or shows some in detail, waiting for them if asked.
pub struct Jobs;

/// `manage_job`: stops, merges, resolves or retries a job.
pub struct ManageJob;

/// The job whose worktree `root` is, when an agent of a job calls.
fn calling_job(store: &JobStore, root: &Path) -> Option<Job> {
    let root = root.canonicalize().ok()?;

    store.list().into_iter().find(|job| {
        job.worktree
            .as_ref()
            .and_then(|worktree| worktree.canonicalize().ok())
            .is_some_and(|worktree| root.starts_with(worktree))
    })
}

fn store_for(context: &ToolContext) -> JobStore {
    JobStore::for_project(context.workspace.root())
}

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(work)
        .await
        .expect("job store work finished")
}

#[async_trait]
impl Tool for SpawnJob {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "spawn_job".to_string(),

            description: "Start a background job: another agent that carries out one \
                          self-contained instruction in parallel with you, in its own git \
                          worktree and branch, so jobs never overwrite each other or your \
                          files. It starts from the project's last commit (uncommitted changes \
                          are not in it), keeps running if the window closes, and when it \
                          finishes well its branch is merged back after a conflict check. Use \
                          it for independent pieces of work; give each a complete \
                          instruction, since the job does not see this conversation. A job \
                          that needs another's result lists it in `depends_on` and starts \
                          after that job is merged. Check on jobs with `jobs`."
                .to_string(),

            input_schema: json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "A few words naming the job." },
                    "instruction": {
                        "type": "string",
                        "description": "Everything the job's agent needs: the goal, the files or area, constraints, and how to check the result.",
                    },
                    "depends_on": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Jobs (such as `job-2`) that must be finished and merged first.",
                    },
                    "auto_merge": {
                        "type": "boolean",
                        "description": "Merge the job's branch as soon as it finishes well (default true). False leaves it for `manage_job` merge.",
                    },
                    "max_steps": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 500,
                        "description": "Model calls the job may make.",
                    },
                },
                "required": ["instruction"],
            }),
        }
    }

    fn access(&self) -> Access {
        Access::Mutating
    }

    fn describe(&self, input: &Value) -> String {
        let title = input
            .get("title")
            .and_then(Value::as_str)
            .or_else(|| input.get("instruction").and_then(Value::as_str))
            .unwrap_or("a job");

        format!("start background job: {}", title.chars().take(80).collect::<String>())
    }

    async fn run(&self, input: Value, context: &ToolContext) -> Result<String, ToolError> {
        let instruction = input
            .get("instruction")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("`instruction` is required".to_string()))?
            .to_string();

        let title = input
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        let depends_on = match input.get("depends_on") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(items)) => items
                .iter()
                .map(|item| {
                    item.as_str().map(String::from).ok_or_else(|| {
                        ToolError::InvalidInput("`depends_on` lists job ids".to_string())
                    })
                })
                .collect::<Result<_, _>>()?,
            Some(_) => {
                return Err(ToolError::InvalidInput(
                    "`depends_on` must be a list of job ids".to_string(),
                ));
            }
        };

        let auto_merge = input
            .get("auto_merge")
            .and_then(Value::as_bool)
            .unwrap_or(true);

        let max_steps = match input.get("max_steps") {
            None | Some(Value::Null) => None,
            Some(value) => Some(
                value
                    .as_u64()
                    .filter(|steps| (1..=500).contains(steps))
                    .ok_or_else(|| {
                        ToolError::InvalidInput("`max_steps` is a number from 1 to 500".into())
                    })? as u32,
            ),
        };

        let root = context.workspace.root().to_path_buf();

        let (job, runner) = blocking(move || {
            let store = JobStore::for_project(&root);
            let parent = calling_job(&store, &root);

            let new = NewJob {
                title,
                instruction,
                depends_on,
                created_by: parent
                    .as_ref()
                    .map(|job| job.id.clone())
                    .unwrap_or_else(|| "agent".to_string()),
                depth: parent.as_ref().map(|job| job.depth + 1).unwrap_or(0),
                auto_merge,
                max_steps,
                folder: jobs::folder_in_repository(&root),
            };

            let job = store.add(new)?;
            let runner = store.ensure_runner();

            Ok::<_, String>((job, runner))
        })
        .await
        .map_err(ToolError::Failed)?;

        let runner = match runner {
            RunnerStart::Running => "A job runner is running and will start it when it can.".to_string(),
            RunnerStart::Started => "A job runner was started for it.".to_string(),
            RunnerStart::Failed(error) => format!(
                "It is queued, but no job runner could be started ({error}). The desktop app starts one when it is open on this project."
            ),
        };

        let after = match job.depends_on.is_empty() {
            true => String::new(),
            false => format!(" It starts after {} are merged.", job.depends_on.join(", ")),
        };

        Ok(format!(
            "Queued {} \"{}\".{after} {runner} Use `jobs` with ids [\"{}\"] to see how it goes, \
             or with `wait_seconds` to wait for it.",
            job.id, job.title, job.id
        ))
    }
}

#[async_trait]
impl Tool for Jobs {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "jobs".to_string(),

            description: "List the project's background jobs, or show the ones in `ids` in \
                          detail: status, branch, files changed, overlaps with other jobs, \
                          conflicts, recent activity and, once finished, the job's report. \
                          With `wait_seconds`, wait until the jobs in `ids` (or all running \
                          and queued jobs) have finished, then show them."
                .to_string(),

            input_schema: json!({
                "type": "object",
                "properties": {
                    "ids": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Jobs to show in detail.",
                    },
                    "wait_seconds": {
                        "type": "integer",
                        "minimum": 0,
                        "maximum": MAX_WAIT.as_secs(),
                        "description": "Wait up to this long for the jobs to finish.",
                    },
                },
            }),
        }
    }

    fn access(&self) -> Access {
        Access::ReadOnly
    }

    async fn run(&self, input: Value, context: &ToolContext) -> Result<String, ToolError> {
        let ids: Vec<String> = input
            .get("ids")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(jobs::normalize_id)
                    .collect()
            })
            .unwrap_or_default();

        let wait = input
            .get("wait_seconds")
            .and_then(Value::as_u64)
            .map(|seconds| Duration::from_secs(seconds).min(MAX_WAIT))
            .unwrap_or_default();

        let store = store_for(context);
        let started = Instant::now();

        loop {
            let store_now = store.clone();
            let all = blocking(move || store_now.list()).await;

            for id in &ids {
                if !all.iter().any(|job| &job.id == id) {
                    return Err(ToolError::InvalidInput(format!("there is no job `{id}`")));
                }
            }

            let watched: Vec<&Job> = match ids.is_empty() {
                true => all.iter().filter(|job| job.status.is_open()).collect(),
                false => all.iter().filter(|job| ids.contains(&job.id)).collect(),
            };

            // A queued job that can never start does not hold the wait.
            let pending = watched.iter().any(|job| {
                job.status == JobStatus::Running
                    || (job.status == JobStatus::Queued
                        && !matches!(JobStore::readiness(job, &all), Readiness::Blocked(_)))
            });

            if wait.is_zero() || !pending || started.elapsed() >= wait {
                let mut text = match ids.is_empty() {
                    true => summary(&all),
                    false => watched
                        .iter()
                        .map(|job| detail(&store, job, &all))
                        .collect::<Vec<_>>()
                        .join("\n\n"),
                };

                if !wait.is_zero() && pending {
                    text.push_str(&format!(
                        "\n\nStill working after {}s of waiting.",
                        started.elapsed().as_secs()
                    ));
                }

                return Ok(text);
            }

            tokio::time::sleep(POLL).await;
        }
    }
}

#[async_trait]
impl Tool for ManageJob {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "manage_job".to_string(),

            description: "Act on a background job. `cancel` stops a queued or running job. \
                          `merge` merges a finished job's branch into the project, after a \
                          conflict check. `resolve` has an agent resolve a conflicted job's \
                          conflicts in its own worktree, then merges it. `retry` queues a \
                          failed or cancelled job again, carrying on where it stopped."
                .to_string(),

            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "The job, such as `job-3`." },
                    "action": {
                        "type": "string",
                        "enum": ["cancel", "merge", "resolve", "retry"],
                    },
                },
                "required": ["id", "action"],
            }),
        }
    }

    fn access(&self) -> Access {
        Access::Mutating
    }

    fn describe(&self, input: &Value) -> String {
        format!(
            "{} {}",
            input.get("action").and_then(Value::as_str).unwrap_or("manage"),
            input.get("id").and_then(Value::as_str).unwrap_or("a job")
        )
    }

    async fn run(&self, input: Value, context: &ToolContext) -> Result<String, ToolError> {
        let id = input
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("`id` is required".to_string()))?
            .to_string();

        let action = input
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("`action` is required".to_string()))?
            .to_string();

        let store = store_for(context);

        let said = blocking(move || {
            let job = match action.as_str() {
                "cancel" => store.request_cancel(&id)?,
                "merge" => store.request_merge(&id)?,
                "resolve" => store.request_resolve(&id)?,
                "retry" => store.retry(&id)?,
                other => return Err(format!("`{other}` is not an action")),
            };

            let done = match action.as_str() {
                "cancel" if job.status == JobStatus::Cancelled => "cancelled".to_string(),
                "cancel" => "is being stopped".to_string(),
                "merge" => "will be merged by the job runner".to_string(),
                "resolve" => "will have its conflicts resolved, then be merged".to_string(),
                _ => "is queued again".to_string(),
            };

            // Everything but a cancel needs a runner to act.
            let runner = match (action.as_str(), store.ensure_runner()) {
                ("cancel", _) | (_, RunnerStart::Running | RunnerStart::Started) => String::new(),
                (_, RunnerStart::Failed(error)) => {
                    format!(" No job runner could be started yet: {error}.")
                }
            };

            Ok(format!("{} {done}.{runner}", job.id))
        })
        .await
        .map_err(ToolError::Failed)?;

        Ok(said)
    }
}

/// One line per job.
fn summary(all: &[Job]) -> String {
    if all.is_empty() {
        return "No background jobs. Start one with `spawn_job`.".to_string();
    }

    let now = jobs::now_ms();

    all.iter()
        .map(|job| {
            let mut line = format!("{} [{}] {}", job.id, state(job, all), job.title);

            if let Some(elapsed) = job.elapsed(now) {
                line.push_str(&format!(" · {}", duration(elapsed)));
            }

            if job.steps > 0 {
                line.push_str(&format!(" · {} steps", job.steps));
            }

            if !job.changed_files.is_empty() {
                line.push_str(&format!(" · {} files", job.changed_files.len()));
            }

            if !job.overlaps.is_empty() {
                let with: Vec<&str> = job.overlaps.iter().map(|overlap| overlap.job.as_str()).collect();

                line.push_str(&format!(" · overlaps {}", with.join(", ")));
            }

            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Everything about one job.
fn detail(store: &JobStore, job: &Job, all: &[Job]) -> String {
    let mut text = format!("{} [{}] {}", job.id, state(job, all), job.title);

    if let Some(branch) = &job.branch {
        text.push_str(&format!("\nBranch: {branch}"));
    }

    if !job.depends_on.is_empty() {
        text.push_str(&format!("\nAfter: {}", job.depends_on.join(", ")));
    }

    if job.steps > 0 {
        text.push_str(&format!("\nSteps: {} · tokens: {}", job.steps, job.tokens));
    }

    if let Some(verified) = job.verified {
        text.push_str(match verified {
            true => "\nChecks: passed",
            false => "\nChecks: failed",
        });
    }

    if !job.changed_files.is_empty() {
        text.push_str(&format!("\nChanged: {}", job.changed_files.join(", ")));
    }

    for overlap in &job.overlaps {
        text.push_str(&format!(
            "\nAlso changed by {}: {}",
            overlap.job,
            overlap.files.join(", ")
        ));
    }

    if !job.conflicts.is_empty() {
        text.push_str(&format!(
            "\nConflicts with the project in: {}. `manage_job` resolve has them resolved in the job's worktree.",
            job.conflicts.join(", ")
        ));
    }

    if let Some(error) = &job.error {
        text.push_str(&format!("\nNote: {error}"));
    }

    if job.status.is_open() {
        let activity = jobs::activity(&store.log_path(&job.id), ACTIVITY_LINES);

        if !activity.is_empty() {
            text.push_str("\nRecent:");

            for item in activity {
                text.push_str(&format!("\n- {}", item.text));
            }
        }
    }

    if !job.summary.trim().is_empty() {
        text.push_str(&format!("\nReport:\n{}", job.summary.trim()));
    }

    text
}

fn state(job: &Job, all: &[Job]) -> String {
    if job.status != JobStatus::Queued {
        return job.status.label().to_string();
    }

    match JobStore::readiness(job, all) {
        Readiness::Ready => "queued".to_string(),
        Readiness::Waiting(on) => format!("waiting on {}", on.join(", ")),
        Readiness::Blocked(by) => format!("blocked: {by} did not finish"),
    }
}

fn duration(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();

    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m {}s", seconds / 60, seconds % 60),
        _ => format!("{}h {}m", seconds / 3600, seconds % 3600 / 60),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cortex_core::{AutoApprove, Workspace};

    use super::*;

    fn context(folder: &Path) -> ToolContext {
        ToolContext {
            workspace: Workspace::new(folder).unwrap(),
            approver: Arc::new(AutoApprove),
        }
    }

    #[tokio::test]
    async fn jobs_are_queued_listed_and_managed_by_the_model() {
        let folder = tempfile::tempdir().unwrap();
        let context = context(folder.path());

        let queued = SpawnJob
            .run(
                json!({ "title": "Parser", "instruction": "Write the parser." }),
                &context,
            )
            .await
            .unwrap();

        assert!(queued.contains("job-1"), "{queued}");

        let second = SpawnJob
            .run(
                json!({ "instruction": "Test it.", "depends_on": ["job-1"] }),
                &context,
            )
            .await
            .unwrap();

        assert!(second.contains("after job-1"), "{second}");

        let listed = Jobs.run(json!({}), &context).await.unwrap();

        assert!(listed.contains("job-1 [queued] Parser"), "{listed}");
        assert!(listed.contains("job-2 [waiting on job-1]"), "{listed}");

        let missing = Jobs.run(json!({ "ids": ["job-9"] }), &context).await;

        assert!(missing.is_err());

        let cancelled = ManageJob
            .run(json!({ "id": "1", "action": "cancel" }), &context)
            .await
            .unwrap();

        assert!(cancelled.contains("cancelled"));

        // Nothing is left that could finish, so a wait returns at once.
        let waited = Jobs
            .run(json!({ "ids": ["job-2"], "wait_seconds": 60 }), &context)
            .await
            .unwrap();

        assert!(waited.contains("blocked"), "{waited}");

        let refused = ManageJob
            .run(json!({ "id": "2", "action": "merge" }), &context)
            .await;

        assert!(refused.is_err());

        let bad = SpawnJob
            .run(json!({ "instruction": "x", "depends_on": "job-1" }), &context)
            .await;

        assert!(matches!(bad, Err(ToolError::InvalidInput(_))));
    }
}
