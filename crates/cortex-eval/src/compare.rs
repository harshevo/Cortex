//! Comparing two runs to decide whether a change helped.

use crate::report::{SuiteReport, Totals, format_cost};

/// Below this many attempts per task, a difference of a task or two is
/// within normal run-to-run variation.
const ATTEMPTS_FOR_CONFIDENCE: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Improved,
    Regressed,
    Unchanged,
}

#[derive(Debug, Clone)]
pub struct Comparison {
    pub baseline_label: String,
    pub candidate_label: String,
    pub baseline: Totals,
    pub candidate: Totals,

    /// How many tasks both runs finished at least once. The totals cover
    /// exactly these.
    pub shared_tasks: usize,

    /// Tasks whose solve rate went up, with the rates before and after.
    pub gained: Vec<(String, f64, f64)>,

    /// Tasks whose solve rate went down.
    pub lost: Vec<(String, f64, f64)>,

    /// Steps and time for each task both runs attempted, in the order
    /// the candidate ran them. Order matters when memory is on: a later
    /// task has more earlier work to draw on.
    pub per_task: String,

    pub verdict: Verdict,

    /// False when either run has too few attempts per task to trust.
    pub confident: bool,
}

impl Comparison {
    pub fn between(baseline: &SuiteReport, candidate: &SuiteReport) -> Self {
        let before = baseline.solve_rate_by_task();
        let after = candidate.solve_rate_by_task();

        let mut gained = Vec::new();
        let mut lost = Vec::new();

        for (task, &rate_after) in &after {
            let Some(&rate_before) = before.get(task) else {
                continue;
            };

            if rate_after > rate_before {
                gained.push((task.clone(), rate_before, rate_after));
            } else if rate_after < rate_before {
                lost.push((task.clone(), rate_before, rate_after));
            }
        }

        // Totals cover only the tasks both runs finished. An attempt lost
        // to a provider error has no steps or tokens, and counting it, or
        // a task only one run finished, would make the runs look
        // different when the agent was not.
        let shared = shared_tasks(baseline, candidate);

        let baseline_totals = restricted_to(baseline, &shared).totals();
        let candidate_totals = restricted_to(candidate, &shared).totals();

        // The share of each task done, which is the solve rate when no
        // task is scored by its parts.
        let delta = candidate_totals.mean_score() - baseline_totals.mean_score();

        let verdict = if delta > f64::EPSILON {
            Verdict::Improved
        } else if delta < -f64::EPSILON {
            Verdict::Regressed
        } else {
            Verdict::Unchanged
        };

        let confident = attempts_per_task(baseline) >= ATTEMPTS_FOR_CONFIDENCE
            && attempts_per_task(candidate) >= ATTEMPTS_FOR_CONFIDENCE;

        Self {
            baseline_label: baseline.label.clone(),
            candidate_label: candidate.label.clone(),
            baseline: baseline_totals,
            candidate: candidate_totals,
            shared_tasks: shared.len(),
            gained,
            lost,
            per_task: per_task_table(baseline, candidate),
            verdict,
            confident,
        }
    }

    pub fn render(&self) -> String {
        let mut text = String::new();

        text.push_str(&format!(
            "Comparing `{}` (baseline) with `{}` (candidate)\n\n",
            self.baseline_label, self.candidate_label
        ));

        text.push_str(&format!(
            "  On the {} tasks both runs finished:\n\n",
            self.shared_tasks
        ));

        text.push_str(&format!(
            "  Solve rate        {:>5.0}%  ->  {:>5.0}%\n",
            self.baseline.solve_rate() * 100.0,
            self.candidate.solve_rate() * 100.0,
        ));

        if self.baseline.graded || self.candidate.graded {
            text.push_str(&format!(
                "  Share of task done{:>5.0}%  ->  {:>5.0}%\n",
                self.baseline.mean_score() * 100.0,
                self.candidate.mean_score() * 100.0,
            ));
        }

        text.push_str(&format!(
            "  Mean steps        {:>6.1}  ->  {:>6.1}\n",
            self.baseline.mean_steps(),
            self.candidate.mean_steps(),
        ));

        text.push_str(&format!(
            "  Mean time         {:>5.0}s  ->  {:>5.0}s\n",
            self.baseline.mean_seconds(),
            self.candidate.mean_seconds(),
        ));

        text.push_str(&format!(
            "  Total tokens      {:>6}  ->  {:>6}\n",
            self.baseline.usage.total(),
            self.candidate.usage.total(),
        ));

        text.push_str(&format!(
            "  Cost per solved   {:>6}  ->  {:>6}\n",
            format_cost(self.baseline.cost_per_solved()),
            format_cost(self.candidate.cost_per_solved()),
        ));

        text.push_str(&self.per_task);

        for (task, before, after) in &self.gained {
            text.push_str(&format!(
                "\n  + {task}: {:.0}% -> {:.0}%",
                before * 100.0,
                after * 100.0
            ));
        }

        for (task, before, after) in &self.lost {
            text.push_str(&format!(
                "\n  - {task}: {:.0}% -> {:.0}%",
                before * 100.0,
                after * 100.0
            ));
        }

        let verdict = match self.verdict {
            Verdict::Improved => "IMPROVED: the candidate solves more.",
            Verdict::Regressed => "REGRESSED: the candidate solves less.",
            Verdict::Unchanged => "UNCHANGED solve rate. Compare cost and steps above.",
        };

        text.push_str(&format!("\n\nVerdict: {verdict}\n"));

        if !self.confident {
            text.push_str(&format!(
                "Caution: fewer than {ATTEMPTS_FOR_CONFIDENCE} attempts per task. Model output \
                 varies between runs, so repeat with `--repeats {ATTEMPTS_FOR_CONFIDENCE}` or \
                 more before trusting this.\n"
            ));
        }

        text
    }
}

/// The tasks with at least one attempt that finished in both runs.
fn shared_tasks(baseline: &SuiteReport, candidate: &SuiteReport) -> Vec<String> {
    let finished = |report: &SuiteReport| report.solve_rate_by_task();

    let in_candidate = finished(candidate);

    finished(baseline)
        .into_keys()
        .filter(|task| in_candidate.contains_key(task))
        .collect()
}

/// A copy of `report` with only the finished attempts at `tasks`.
fn restricted_to(report: &SuiteReport, tasks: &[String]) -> SuiteReport {
    let mut restricted = report.clone();

    restricted
        .results
        .retain(|result| result.error.is_none() && tasks.contains(&result.task_id));

    restricted
}

/// Mean steps and seconds of the attempts at `task` that finished.
fn task_means(report: &SuiteReport, task: &str) -> Option<(f64, f64)> {
    let attempts: Vec<_> = report
        .results
        .iter()
        .filter(|result| result.task_id == task && result.error.is_none())
        .collect();

    if attempts.is_empty() {
        return None;
    }

    let count = attempts.len() as f64;

    let steps: f64 = attempts.iter().map(|result| f64::from(result.steps)).sum();
    let seconds: f64 = attempts
        .iter()
        .map(|result| result.duration_ms as f64 / 1000.0)
        .sum();

    Some((steps / count, seconds / count))
}

fn per_task_table(baseline: &SuiteReport, candidate: &SuiteReport) -> String {
    let mut order: Vec<&str> = Vec::new();

    for result in &candidate.results {
        if !order.contains(&result.task_id.as_str()) {
            order.push(&result.task_id);
        }
    }

    let mut text = String::new();

    for (position, task) in order.iter().enumerate() {
        let (Some((steps_before, seconds_before)), Some((steps_after, seconds_after))) =
            (task_means(baseline, task), task_means(candidate, task))
        else {
            continue;
        };

        if text.is_empty() {
            text.push_str("\n  Per task, in the order the candidate ran them:\n");
            text.push_str("    #   task                          steps          time\n");
        }

        text.push_str(&format!(
            "    {:<3} {:<28} {:>4.0} -> {:<4.0}  {:>5.0}s -> {:.0}s\n",
            position + 1,
            task,
            steps_before,
            steps_after,
            seconds_before,
            seconds_after,
        ));
    }

    text
}

fn attempts_per_task(report: &SuiteReport) -> usize {
    let tasks = report.solve_rate_by_task().len();

    if tasks == 0 {
        return 0;
    }

    report.results.len() / tasks
}
