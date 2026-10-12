//! Checking that the tasks themselves are sound, without any model.
//!
//! A task is only a fair measurement if its check fails on the untouched
//! project and passes on the reference solution.

use anyhow::Result;

use crate::check;
use crate::task::{Suite, Task};
use crate::workdir;

#[derive(Debug, Clone)]
pub struct TaskValidation {
    pub task_id: String,

    /// The check fails before any change, as it must.
    pub fails_untouched: bool,

    /// The check passes with `solution/` applied. `None` when the task has
    /// no reference solution.
    pub passes_with_solution: Option<bool>,

    /// Parts of the task that already pass before any change. Such a part
    /// would be credited to an agent that did nothing.
    pub parts_passing_untouched: Vec<String>,

    /// Parts that still fail with the reference solution applied.
    pub parts_failing_with_solution: Vec<String>,
}

impl TaskValidation {
    pub fn is_sound(&self) -> bool {
        self.fails_untouched
            && self.passes_with_solution == Some(true)
            && self.parts_passing_untouched.is_empty()
            && self.parts_failing_with_solution.is_empty()
    }
}

pub async fn validate_suite(suite: &Suite) -> Result<Vec<TaskValidation>> {
    let mut validations = Vec::new();

    for task in &suite.tasks {
        validations.push(validate_task(task).await?);
    }

    Ok(validations)
}

async fn validate_task(task: &Task) -> Result<TaskValidation> {
    let untouched = workdir::prepare(task)?;

    let before = check::run(task, untouched.path()).await?;

    let names = |passed: bool, milestones: &[check::MilestoneResult]| -> Vec<String> {
        milestones
            .iter()
            .filter(|milestone| milestone.passed == passed)
            .map(|milestone| milestone.name.clone())
            .collect()
    };

    let mut parts_failing_with_solution = Vec::new();

    let passes_with_solution = if task.solution_dir().is_dir() {
        let solved = workdir::prepare(task)?;

        workdir::copy_tree(&task.solution_dir(), solved.path())?;

        let after = check::run(task, solved.path()).await?;

        parts_failing_with_solution = names(false, &after.milestones);

        Some(after.passed)
    } else {
        None
    };

    Ok(TaskValidation {
        task_id: task.id.clone(),
        fails_untouched: !before.passed,
        passes_with_solution,
        parts_passing_untouched: names(true, &before.milestones),
        parts_failing_with_solution,
    })
}
