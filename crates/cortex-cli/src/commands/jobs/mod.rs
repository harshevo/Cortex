//! `cortex jobs`: background jobs, and the runner that carries them out.

mod approver;
pub mod runner;

use std::process::ExitCode;

use anyhow::{Result, bail};
use cortex_core::jobs::{self, JobStatus, JobStore, NewJob, Readiness, RunnerStart};
use cortex_core::merge;

use crate::args::{JobsArgs, JobsCommand};

pub async fn execute(args: JobsArgs) -> Result<ExitCode> {
    match args.command {
        JobsCommand::List(args) => {
            let store = JobStore::for_project(&args.dir);
            let all = store.list();

            if args.json {
                println!("{}", serde_json::to_string_pretty(&all)?);
            } else if all.is_empty() {
                println!("No jobs.");
            } else {
                for job in &all {
                    println!("{:<8} {:<22} {}", job.id, status(job, &all), job.title);
                }
            }
        }

        JobsCommand::Show(args) => {
            let store = JobStore::for_project(&args.dir);
            let Some(job) = store.get(&jobs::normalize_id(&args.id)) else {
                bail!("there is no job `{}`", args.id);
            };

            let all = store.list();

            println!("{} · {} · {}", job.id, status(&job, &all), job.title);

            if let Some(branch) = &job.branch {
                println!("branch   {branch}");
            }

            if let Some(worktree) = &job.worktree {
                println!("worktree {}", worktree.display());
            }

            if !job.changed_files.is_empty() {
                println!("changed  {}", job.changed_files.join(", "));
            }

            for overlap in &job.overlaps {
                println!("overlaps {}: {}", overlap.job, overlap.files.join(", "));
            }

            if !job.conflicts.is_empty() {
                println!("conflict {}", job.conflicts.join(", "));
            }

            if let Some(error) = &job.error {
                println!("note     {error}");
            }

            println!("\n{}", job.instruction);

            let activity = jobs::activity(&store.log_path(&job.id), 12);

            if !activity.is_empty() {
                println!();

                for item in activity {
                    println!("  {}", item.text);
                }
            }

            if !job.summary.trim().is_empty() {
                println!("\n{}", job.summary.trim());
            }
        }

        JobsCommand::Add(args) => {
            let store = JobStore::for_project(&args.dir);

            let job = store
                .add(NewJob {
                    title: args.title.unwrap_or_default(),
                    instruction: args.instruction,
                    depends_on: args.after,
                    created_by: "person".to_string(),
                    depth: 0,
                    auto_merge: !args.no_merge,
                    max_steps: args.max_steps,
                    folder: jobs::folder_in_repository(&args.dir),
                })
                .map_err(anyhow::Error::msg)?;

            println!("Queued {}: {}", job.id, job.title);

            say_runner(store.ensure_runner());
        }

        JobsCommand::Cancel(args) => {
            let store = JobStore::for_project(&args.dir);
            let job = store.request_cancel(&args.id).map_err(anyhow::Error::msg)?;

            match job.status {
                JobStatus::Cancelled => println!("Cancelled {}.", job.id),
                _ => println!("Stopping {}.", job.id),
            }
        }

        JobsCommand::Merge(args) => {
            let store = JobStore::for_project(&args.dir);
            let job = store.request_merge(&args.id).map_err(anyhow::Error::msg)?;

            println!("{} will be merged by the runner.", job.id);

            say_runner(store.ensure_runner());
        }

        JobsCommand::Resolve(args) => {
            let store = JobStore::for_project(&args.dir);
            let job = store.request_resolve(&args.id).map_err(anyhow::Error::msg)?;

            println!("{} will have its conflicts resolved.", job.id);

            say_runner(store.ensure_runner());
        }

        JobsCommand::Retry(args) => {
            let store = JobStore::for_project(&args.dir);
            let job = store.retry(&args.id).map_err(anyhow::Error::msg)?;

            println!("Queued {} again.", job.id);

            say_runner(store.ensure_runner());
        }

        JobsCommand::Remove(args) => {
            let store = JobStore::for_project(&args.job.dir);

            remove(&store, &args.job.id, args.force)
                .await
                .map_err(anyhow::Error::msg)?;
        }

        JobsCommand::Serve(args) => return runner::serve(args).await,
    }

    Ok(ExitCode::SUCCESS)
}

/// Forgets a job and removes its worktree and branch. The branch of a
/// job whose work was never merged is kept unless `force`.
pub async fn remove(store: &JobStore, id: &str, force: bool) -> Result<String, String> {
    let id = jobs::normalize_id(id);

    let job = store
        .get(&id)
        .ok_or_else(|| format!("there is no job `{id}`"))?;

    if job.status.is_open() {
        return Err(format!("{} is {}; stop it first", job.id, job.status.label()));
    }

    let unmerged = job.status != JobStatus::Merged && job.branch.is_some();

    if let Some(worktree) = &job.worktree {
        let branch = match (unmerged, force) {
            (true, false) => None,
            _ => job.branch.as_deref(),
        };

        merge::remove_worktree(store.root(), worktree, branch, force).await?;
    }

    store.remove(&id)?;

    let said = match (unmerged && !force, &job.branch) {
        (true, Some(branch)) => {
            format!("Removed {}. Its unmerged branch {branch} is kept.", job.id)
        }
        _ => format!("Removed {}.", job.id),
    };

    println!("{said}");

    Ok(said)
}

fn say_runner(start: RunnerStart) {
    match start {
        RunnerStart::Running => println!("A runner is carrying out this project's jobs."),
        RunnerStart::Started => println!("Started a runner for this project's jobs."),
        RunnerStart::Failed(error) => println!("No runner was started: {error}"),
    }
}

fn status(job: &jobs::Job, all: &[jobs::Job]) -> String {
    if job.status != JobStatus::Queued {
        return job.status.label().to_string();
    }

    match JobStore::readiness(job, all) {
        Readiness::Ready => "queued".to_string(),
        Readiness::Waiting(on) => format!("after {}", on.join(", ")),
        Readiness::Blocked(by) => format!("blocked by {by}"),
    }
}
