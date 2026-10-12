//! Scoring a finished working copy.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use cortex_core::process;
use serde::{Deserialize, Serialize};

use crate::task::Task;
use crate::workdir;

#[derive(Debug, Clone)]
pub struct CheckResult {
    pub passed: bool,
    pub exit_code: Option<i32>,
    pub output: String,

    /// How each part of a long task did. Empty when the task has none.
    pub milestones: Vec<MilestoneResult>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MilestoneResult {
    pub name: String,
    pub passed: bool,
    pub weight: f64,
}

/// The weighted share of `milestones` passed, from 0 to 1.
pub fn score(milestones: &[MilestoneResult]) -> f64 {
    let total: f64 = milestones.iter().map(|milestone| milestone.weight).sum();

    if total <= 0.0 {
        return 0.0;
    }

    let passed: f64 = milestones
        .iter()
        .filter(|milestone| milestone.passed)
        .map(|milestone| milestone.weight)
        .sum();

    passed / total
}

/// Copies the hidden files over the working copy, then runs the check.
///
/// The overlay happens here, after the agent is done, so the agent cannot
/// read the scoring tests, and any edits it made to a file of the same name
/// are replaced by the original.
pub async fn run(task: &Task, workdir: &Path) -> Result<CheckResult> {
    workdir::copy_tree(&task.hidden_dir(), workdir)
        .with_context(|| format!("could not add hidden files for task `{}`", task.id))?;

    let timeout = Duration::from_secs(task.check.timeout_seconds);

    let output = process::run_shell(&task.check.command, workdir, timeout)
        .await
        .with_context(|| format!("could not run the check for task `{}`", task.id))?;

    let combined = format!("{}{}", output.stdout, output.stderr);

    let mut milestones = Vec::new();

    for milestone in &task.milestones {
        let timeout = Duration::from_secs(milestone.timeout_seconds);

        let output = process::run_shell(&milestone.command, workdir, timeout)
            .await
            .with_context(|| {
                format!("could not check `{}` of task `{}`", milestone.name, task.id)
            })?;

        milestones.push(MilestoneResult {
            name: milestone.name.clone(),
            passed: output.succeeded(),
            weight: milestone.weight,
        });
    }

    Ok(CheckResult {
        passed: output.succeeded(),
        exit_code: output.exit_code,
        output: combined,
        milestones,
    })
}
