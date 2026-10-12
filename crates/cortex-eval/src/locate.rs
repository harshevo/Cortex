//! Measuring, without a model, whether the context put in front of a
//! task points at the right files.
//!
//! Every task has a reference solution, so the files that have to change
//! are known. A context source (memory, the router, the ranking of likely
//! files) is asked for its briefing on the untouched project, and the
//! briefing is read for file paths in the order they are mentioned. The
//! position of the first right one is the score.
//!
//! This costs no model calls, so a change to a context source can be
//! measured in seconds before it is given a full evaluation run.

use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use cortex_core::{ContextSource, Workspace};
use cortex_index::walk;

use crate::task::{Suite, Task};
use crate::workdir;

#[derive(Debug, Clone)]
pub struct Located {
    pub task_id: String,

    /// The files the reference solution changes.
    pub wanted: Vec<String>,

    /// Where the first of them comes among the files the briefing
    /// mentions, counting from 1. `None` when it mentions none of them.
    pub rank: Option<usize>,

    /// Project files the briefing mentions.
    pub mentioned: usize,

    /// Length of the briefing: what it costs on every step of the run.
    pub chars: usize,

    pub duration_ms: u64,
}

/// How a set of context sources did over a suite.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Summary {
    pub tasks: usize,
    pub first: usize,
    pub in_three: usize,
    pub in_five: usize,
    pub mean_chars: usize,
    pub mean_ms: u64,
}

impl Summary {
    pub fn of(located: &[Located]) -> Self {
        let within = |limit: usize| {
            located
                .iter()
                .filter(|task| task.rank.is_some_and(|rank| rank <= limit))
                .count()
        };

        let tasks = located.len().max(1);

        Self {
            tasks: located.len(),
            first: within(1),
            in_three: within(3),
            in_five: within(5),
            mean_chars: located.iter().map(|task| task.chars).sum::<usize>() / tasks,
            mean_ms: located.iter().map(|task| task.duration_ms).sum::<u64>() / tasks as u64,
        }
    }
}

/// Asks `sources` about every task of `suite` that has a solution.
pub async fn locate_suite(
    suite: &Suite,
    sources: &[Arc<dyn ContextSource>],
) -> Result<Vec<Located>> {
    let mut located = Vec::new();

    for task in &suite.tasks {
        if let Some(found) = locate_task(task, sources).await? {
            located.push(found);
        }
    }

    Ok(located)
}

async fn locate_task(task: &Task, sources: &[Arc<dyn ContextSource>]) -> Result<Option<Located>> {
    let wanted = relative_files(&task.solution_dir());

    if wanted.is_empty() {
        return Ok(None);
    }

    let workdir = workdir::prepare(task)?;
    let workspace = Workspace::new(workdir.path())?;

    let started = Instant::now();

    let mut text = String::new();

    for source in sources {
        if let Some(briefing) = source.brief(&task.instruction, &workspace).await {
            text.push_str(&briefing.text);
            text.push('\n');
        }
    }

    let duration_ms = started.elapsed().as_millis() as u64;

    let mentioned = mentioned_files(&text, &relative_files(workspace.root()));

    Ok(Some(Located {
        task_id: task.id.clone(),
        rank: mentioned
            .iter()
            .position(|path| wanted.contains(path))
            .map(|position| position + 1),
        wanted,
        mentioned: mentioned.len(),
        chars: text.chars().count(),
        duration_ms,
    }))
}

/// The paths of `files` that appear in `text`, in the order they first
/// appear.
fn mentioned_files(text: &str, files: &[String]) -> Vec<String> {
    let mut found: Vec<(usize, &String)> = files
        .iter()
        .filter_map(|path| Some((text.find(path.as_str())?, path)))
        .collect();

    found.sort();

    found.into_iter().map(|(_, path)| path.clone()).collect()
}

fn relative_files(root: &std::path::Path) -> Vec<String> {
    walk::files_under(root)
        .iter()
        .filter_map(|path| path.strip_prefix(root).ok())
        .map(|path| path.display().to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_are_ranked_by_where_the_briefing_first_names_them() {
        let files = vec![
            "src/calendar.ts".to_string(),
            "src/money.ts".to_string(),
            "src/unused.ts".to_string(),
        ];

        let text = "- src/money.ts (mentions: loss)\n- src/calendar.ts\n- src/money.ts again";

        assert_eq!(
            mentioned_files(text, &files),
            ["src/money.ts", "src/calendar.ts"]
        );
    }

    #[test]
    fn the_summary_counts_tasks_found_within_each_cutoff() {
        let task = |rank: Option<usize>| Located {
            task_id: "task".to_string(),
            wanted: Vec::new(),
            rank,
            mentioned: 5,
            chars: 300,
            duration_ms: 10,
        };

        let summary = Summary::of(&[task(Some(1)), task(Some(3)), task(Some(4)), task(None)]);

        assert_eq!(
            summary,
            Summary {
                tasks: 4,
                first: 1,
                in_three: 2,
                in_five: 3,
                mean_chars: 300,
                mean_ms: 10,
            }
        );
    }
}
