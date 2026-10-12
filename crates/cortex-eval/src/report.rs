//! Results of a suite run, and how they are saved and shown.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use cortex_core::{Outcome, Usage};
use serde::{Deserialize, Serialize};

use crate::check::{self, MilestoneResult};

pub const REPORT_FILE: &str = "report.json";
pub const SUMMARY_FILE: &str = "summary.md";

/// One attempt at one task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    pub task_id: String,
    pub attempt: u32,

    /// The check passed after the agent finished.
    pub solved: bool,

    /// How the agent run ended. `None` when it failed before finishing.
    pub outcome: Option<Outcome>,

    /// An infrastructure failure (for example an API outage). Such attempts
    /// say nothing about the agent and are reported separately.
    pub error: Option<String>,

    pub steps: u32,
    pub usage: Usage,
    pub cost_usd: Option<f64>,
    pub duration_ms: u64,

    /// What the verification gate concluded from the project's own checks.
    /// `None` when the gate was off. Compare with `solved`, which comes
    /// from the hidden checks, to see how far the gate can be trusted.
    #[serde(default)]
    pub verified: Option<bool>,

    /// Times the attempt was carried on after a provider failure.
    #[serde(default)]
    pub resumes: u32,

    /// How each separately checked part of a long task did. Empty for a
    /// task that is only solved or not.
    #[serde(default)]
    pub milestones: Vec<MilestoneResult>,
}

impl TaskResult {
    /// How much of the task was done, from 0 to 1: the share of its parts
    /// for a long task, otherwise all or nothing.
    pub fn score(&self) -> f64 {
        if self.milestones.is_empty() {
            return f64::from(u8::from(self.solved));
        }

        check::score(&self.milestones)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuiteReport {
    pub run_id: String,
    pub label: String,
    pub suite: String,
    pub provider: String,
    pub model: String,
    pub created_unix_ms: u64,
    pub results: Vec<TaskResult>,
}

#[derive(Debug, Clone, Default)]
pub struct Totals {
    pub attempts: usize,
    pub solved: usize,
    pub errored: usize,

    /// The scores of the scored attempts, added up.
    pub score: f64,

    /// Whether any task was scored by its parts.
    pub graded: bool,
    pub usage: Usage,
    /// `None` when the model has no known price (a local or free route).
    pub cost_usd: Option<f64>,
    /// Steps and time of every attempt, including those that ended in an
    /// infrastructure error.
    pub steps: u64,
    pub duration_ms: u64,

    /// Steps and time of the scored attempts only. The means are taken
    /// from these: an attempt cut short by an outage says nothing about
    /// how long the agent takes.
    pub scored_steps: u64,
    pub scored_duration_ms: u64,

    /// Attempts that were carried on after a provider failure, and how
    /// many times in all.
    pub resumed: usize,
    pub resumes: u64,
}

impl Totals {
    /// Share of attempts solved, ignoring attempts that hit an
    /// infrastructure error.
    pub fn solve_rate(&self) -> f64 {
        let scored = self.scored();

        if scored == 0 {
            return 0.0;
        }

        self.solved as f64 / scored as f64
    }

    /// Mean share of a task done, over scored attempts. The same as the
    /// solve rate when no task has parts.
    pub fn mean_score(&self) -> f64 {
        let scored = self.scored();

        if scored == 0 {
            return 0.0;
        }

        self.score / scored as f64
    }

    /// Attempts that ran to an end and were checked.
    pub fn scored(&self) -> usize {
        self.attempts - self.errored
    }

    /// Mean time of a scored attempt.
    pub fn mean_seconds(&self) -> f64 {
        if self.scored() == 0 {
            return 0.0;
        }

        self.scored_duration_ms as f64 / 1000.0 / self.scored() as f64
    }

    /// Mean steps of a scored attempt.
    pub fn mean_steps(&self) -> f64 {
        if self.scored() == 0 {
            return 0.0;
        }

        self.scored_steps as f64 / self.scored() as f64
    }

    /// Dollars spent per solved attempt, the number the project is trying
    /// to push down.
    pub fn cost_per_solved(&self) -> Option<f64> {
        if self.solved == 0 {
            return None;
        }

        Some(self.cost_usd? / self.solved as f64)
    }
}

impl SuiteReport {
    pub fn totals(&self) -> Totals {
        let mut totals = Totals::default();

        for result in &self.results {
            totals.attempts += 1;
            totals.solved += usize::from(result.solved);
            totals.errored += usize::from(result.error.is_some());
            totals.usage += result.usage;

            if let Some(cost) = result.cost_usd {
                totals.cost_usd = Some(totals.cost_usd.unwrap_or_default() + cost);
            }

            totals.steps += u64::from(result.steps);
            totals.duration_ms += result.duration_ms;

            if result.error.is_none() {
                totals.score += result.score();
                totals.graded |= !result.milestones.is_empty();

                totals.scored_steps += u64::from(result.steps);
                totals.scored_duration_ms += result.duration_ms;
            }

            totals.resumed += usize::from(result.resumes > 0);
            totals.resumes += u64::from(result.resumes);
        }

        totals
    }

    /// Share of attempts solved for each task.
    pub fn solve_rate_by_task(&self) -> BTreeMap<String, f64> {
        let mut counts: BTreeMap<String, (u32, u32)> = BTreeMap::new();

        for result in self.results.iter().filter(|result| result.error.is_none()) {
            let (solved, attempts) = counts.entry(result.task_id.clone()).or_default();

            *solved += u32::from(result.solved);
            *attempts += 1;
        }

        counts
            .into_iter()
            .map(|(task, (solved, attempts))| (task, f64::from(solved) / f64::from(attempts)))
            .collect()
    }

    pub fn save(&self, run_dir: &Path) -> Result<()> {
        fs::create_dir_all(run_dir)
            .with_context(|| format!("could not create {}", run_dir.display()))?;

        let json = serde_json::to_string_pretty(self)?;

        fs::write(run_dir.join(REPORT_FILE), json)?;
        fs::write(run_dir.join(SUMMARY_FILE), self.to_markdown())?;

        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("could not read {}", path.display()))?;

        serde_json::from_str(&text).with_context(|| format!("could not parse {}", path.display()))
    }

    pub fn to_markdown(&self) -> String {
        let totals = self.totals();

        let mut text = String::new();

        text.push_str(&format!("# Eval report: {}\n\n", self.label));

        text.push_str(&format!(
            "- Suite: `{}`\n- Model: `{}` ({})\n- Run: `{}`\n\n",
            self.suite, self.model, self.provider, self.run_id
        ));

        text.push_str(
            "| Task | Attempt | Result | Steps | Input tokens | Output tokens | Cost | Time |\n",
        );
        text.push_str("|---|---|---|---|---|---|---|---|\n");

        for result in &self.results {
            text.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} | {} | {} |\n",
                result.task_id,
                result.attempt,
                describe_result(result),
                result.steps,
                result.usage.total_input(),
                result.usage.output_tokens,
                format_cost(result.cost_usd),
                format_seconds(result.duration_ms),
            ));
        }

        text.push_str("\n## Totals\n\n");

        text.push_str(&format!(
            "- Solved: {} of {} attempts ({:.0}%)\n",
            totals.solved,
            totals.attempts - totals.errored,
            totals.solve_rate() * 100.0,
        ));

        if totals.graded {
            text.push_str(&format!(
                "- Mean share of a task done: {:.0}%\n",
                totals.mean_score() * 100.0,
            ));
        }

        if totals.errored > 0 {
            text.push_str(&format!(
                "- Not scored because of infrastructure errors: {}\n",
                totals.errored
            ));
        }

        if totals.resumed > 0 {
            text.push_str(&format!(
                "- Carried on after a provider failure: {} attempts, {} times in all\n",
                totals.resumed, totals.resumes
            ));
        }

        text.push_str(&format!(
            "- Mean steps per scored attempt: {:.1}\n",
            totals.mean_steps()
        ));

        text.push_str(&format!(
            "- Tokens: {} input, {} output\n",
            totals.usage.total_input(),
            totals.usage.output_tokens,
        ));

        text.push_str(&format!(
            "- Estimated cost: {}\n",
            format_cost(totals.cost_usd)
        ));

        text.push_str(&format!(
            "- Estimated cost per solved attempt: {}\n",
            format_cost(totals.cost_per_solved()),
        ));

        text.push_str(&format!(
            "- Total time: {}\n",
            format_seconds(totals.duration_ms)
        ));

        text
    }
}

pub fn describe_result(result: &TaskResult) -> String {
    if result.error.is_some() {
        return "error".to_string();
    }

    let verdict = match (result.solved, result.verified) {
        (true, Some(false)) => "solved (gate said no)",
        (false, Some(true)) => "failed (gate said yes)",
        (true, _) => "solved",
        (false, _) => "failed",
    };

    let verdict = if result.milestones.is_empty() || result.solved {
        verdict.to_string()
    } else {
        let done = result
            .milestones
            .iter()
            .filter(|milestone| milestone.passed)
            .count();

        format!("{verdict}, {done} of {} parts", result.milestones.len())
    };

    match result.outcome {
        Some(Outcome::Completed) | None => verdict,
        Some(Outcome::StepLimit) => format!("{verdict} (step limit)"),
        Some(Outcome::Truncated) => format!("{verdict} (output limit)"),
        Some(Outcome::Refused) => format!("{verdict} (refused)"),
    }
}

pub fn format_cost(cost: Option<f64>) -> String {
    match cost {
        Some(cost) => format!("${cost:.4}"),
        None => "n/a".to_string(),
    }
}

pub fn format_seconds(duration_ms: u64) -> String {
    format!("{:.1}s", duration_ms as f64 / 1000.0)
}
