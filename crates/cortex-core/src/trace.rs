//! Reading a run back to find out what went wrong.
//!
//! A run's log ([`crate::trajectory`]) records every model response, tool
//! call and result, approval, escalation and check. This module turns a
//! log into answers:
//!
//! - [`summarize`]: what a run did, at a glance: its tools, failures,
//!   spending, approvals and how it ended.
//! - [`diverge`]: where two runs of the same task parted ways, as the
//!   first action one took that the other did not. Comparing a run that
//!   passed with one that failed, or one model with another, shows the
//!   decision that made the difference.
//! - [`replay_tools`]: the run's tool calls made again, in order, in a
//!   copy of the project, each output held against the recorded one. The
//!   first that differs is where the environment or the code has moved on
//!   since; when none differ, a failure can be reproduced without a model.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use crate::agent::Outcome;
use crate::message::ToolCall;
use crate::tool::{ToolContext, ToolRegistry};
use crate::trajectory::Event;

/// The events of a log, skipping lines that do not parse.
pub fn read(path: &Path) -> io::Result<Vec<Event>> {
    let text = std::fs::read_to_string(path)?;

    Ok(text
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect())
}

/// The runs in a log, each from its `RunStarted` on.
pub fn runs(events: &[Event]) -> Vec<&[Event]> {
    let starts: Vec<usize> = events
        .iter()
        .enumerate()
        .filter(|(_, event)| matches!(event, Event::RunStarted { .. }))
        .map(|(index, _)| index)
        .collect();

    starts
        .iter()
        .enumerate()
        .map(|(position, start)| {
            let end = starts.get(position + 1).copied().unwrap_or(events.len());

            &events[*start..end]
        })
        .collect()
}

/// One tool call of a run.
#[derive(Debug, Clone, PartialEq)]
pub struct Action {
    pub step: u32,
    pub tool: String,

    /// The input, shortened, for showing.
    pub input: String,

    /// Compared between runs: the tool and its whole input.
    key: String,

    pub failed: bool,
}

impl Action {
    pub fn describe(&self) -> String {
        let mark = if self.failed { " (failed)" } else { "" };

        format!("step {} · {} {}{mark}", self.step, self.tool, self.input)
    }
}

/// The tool calls of `events`, in order.
pub fn actions(events: &[Event]) -> Vec<Action> {
    let mut actions: Vec<Action> = Vec::new();
    let mut by_id = BTreeMap::new();

    for event in events {
        match event {
            Event::ToolStarted { step, call } => {
                by_id.insert(call.id.clone(), actions.len());

                actions.push(Action {
                    step: *step,
                    tool: call.name.clone(),
                    input: clip(&call.input.to_string(), 100),
                    key: format!("{} {}", call.name, call.input),
                    failed: false,
                });
            }

            Event::ToolFinished { tool, result, .. } => {
                if let Some(index) = by_id.get(&result.call_id) {
                    actions[*index].failed =
                        crate::stuck::failed(tool, &result.output, result.is_error);
                }
            }

            _ => {}
        }
    }

    actions
}

/// What a run did, at a glance.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Summary {
    pub instruction: String,
    pub model: String,
    pub outcome: Option<Outcome>,
    pub verified: Option<bool>,
    pub steps: u32,
    pub tokens: u64,
    pub duration_ms: u64,

    /// Calls per tool.
    pub tools: BTreeMap<String, u32>,
    pub failures: u32,

    /// Approvals given and refused.
    pub approved: u32,
    pub refused: u32,

    /// Models moved to.
    pub escalations: Vec<String>,

    /// The largest share of the budget used, when one was set.
    pub budget: Option<f64>,

    pub checks_run: u32,
    pub checks_failed: u32,
}

/// A summary of `run`, the events of one run.
pub fn summarize(run: &[Event]) -> Summary {
    let mut summary = Summary::default();

    for event in run {
        match event {
            Event::RunStarted {
                instruction, model, ..
            } => {
                summary.instruction = clip(instruction.rsplit("## Task\n\n").next().unwrap_or(instruction), 160);
                summary.model = model.clone();
            }

            Event::ToolStarted { call, .. } => {
                *summary.tools.entry(call.name.clone()).or_default() += 1;
            }

            Event::ApprovalDecided { approved, .. } => match approved {
                true => summary.approved += 1,
                false => summary.refused += 1,
            },

            Event::Escalated { to, .. } => summary.escalations.push(to.clone()),

            Event::BudgetReached { share, .. } => {
                summary.budget = Some(summary.budget.unwrap_or(0.0).max(*share));
            }

            Event::VerificationRan { passed, .. } => {
                summary.checks_run += 1;

                if !passed {
                    summary.checks_failed += 1;
                }
            }

            Event::RunFinished {
                outcome,
                steps,
                usage,
                duration_ms,
                verified,
            } => {
                summary.outcome = Some(*outcome);
                summary.steps = *steps;
                summary.tokens = usage.total();
                summary.duration_ms = *duration_ms;
                summary.verified = *verified;
            }

            _ => {}
        }
    }

    summary.failures = actions(run).iter().filter(|action| action.failed).count() as u32;

    summary
}

impl Summary {
    pub fn render(&self) -> String {
        let ending = match (self.outcome, self.verified) {
            (None, _) => "did not finish".to_string(),
            (Some(Outcome::Completed), Some(true)) => "completed, checks passed".to_string(),
            (Some(Outcome::Completed), Some(false)) => "completed, checks failing".to_string(),
            (Some(outcome), _) => format!("{outcome:?}").to_lowercase(),
        };

        let mut tools: Vec<(&String, &u32)> = self.tools.iter().collect();
        tools.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));

        let tools = tools
            .iter()
            .take(8)
            .map(|(tool, count)| format!("{tool} ×{count}"))
            .collect::<Vec<_>>()
            .join(", ");

        let mut lines = vec![
            format!("Task: {}", self.instruction),
            format!("Model: {} · {ending}", self.model),
            format!(
                "{} steps · {} tokens · {:.1}s",
                self.steps,
                self.tokens,
                self.duration_ms as f64 / 1000.0
            ),
            format!("Tools: {tools}"),
            format!("Failed calls: {}", self.failures),
        ];

        if self.approved + self.refused > 0 {
            lines.push(format!(
                "Approvals: {} given, {} refused",
                self.approved, self.refused
            ));
        }

        if !self.escalations.is_empty() {
            lines.push(format!("Escalated to: {}", self.escalations.join(", ")));
        }

        if let Some(share) = self.budget {
            lines.push(format!("Budget used: {:.0}%", share * 100.0));
        }

        if self.checks_run > 0 {
            lines.push(format!(
                "Checks: run {} times, failed {}",
                self.checks_run, self.checks_failed
            ));
        }

        lines.join("\n")
    }
}

/// Where two runs parted ways.
#[derive(Debug, Clone, PartialEq)]
pub struct Divergence {
    /// Actions both took, in the same order, before they differed.
    pub shared: usize,

    /// What each did next; `None` when it had stopped.
    pub left: Option<Action>,
    pub right: Option<Action>,
}

pub fn diverge(left: &[Action], right: &[Action]) -> Divergence {
    let shared = left
        .iter()
        .zip(right)
        .take_while(|(a, b)| a.key == b.key)
        .count();

    Divergence {
        shared,
        left: left.get(shared).cloned(),
        right: right.get(shared).cloned(),
    }
}

/// Two runs side by side and where they parted.
pub fn compare(left: &[Event], right: &[Event]) -> String {
    let (a, b) = (summarize(left), summarize(right));
    let divergence = diverge(&actions(left), &actions(right));

    let next = |action: &Option<Action>| {
        action
            .as_ref()
            .map_or("stopped".to_string(), Action::describe)
    };

    let same = match divergence.shared {
        0 => "They differed from the first call.".to_string(),
        shared => format!("They made the same {shared} calls first."),
    };

    format!(
        "A\n{}\n\nB\n{}\n\n{same}\nThen A: {}\nThen B: {}",
        a.render(),
        b.render(),
        next(&divergence.left),
        next(&divergence.right)
    )
}

/// One call made again.
#[derive(Debug, Clone)]
pub struct Replayed {
    pub step: u32,
    pub tool: String,
    pub recorded: String,
    pub now: String,
}

impl Replayed {
    pub fn same(&self) -> bool {
        self.recorded.trim() == self.now.trim()
    }
}

/// Makes the calls of `run` again with `tools`, in order, and stops at
/// the first whose output differs from the recorded one. `context`
/// should be a copy of the project as it was when the run started.
///
/// Returns every call made, the last being the one that differed when
/// one did.
pub async fn replay_tools(
    run: &[Event],
    tools: &ToolRegistry,
    context: &ToolContext,
) -> Vec<Replayed> {
    let mut calls: Vec<(u32, ToolCall)> = Vec::new();
    let mut recorded: BTreeMap<String, String> = BTreeMap::new();

    for event in run {
        match event {
            Event::ToolStarted { step, call } => calls.push((*step, call.clone())),

            Event::ToolFinished { result, .. } => {
                recorded.insert(result.call_id.clone(), result.output.clone());
            }

            _ => {}
        }
    }

    let mut replayed = Vec::new();

    for (step, call) in calls {
        let Some(before) = recorded.remove(&call.id) else {
            continue;
        };

        let result = tools.dispatch(&call, context).await;

        let done = Replayed {
            step,
            tool: call.name.clone(),
            recorded: before,
            now: result.output,
        };

        let differs = !done.same();
        replayed.push(done);

        if differs {
            break;
        }
    }

    replayed
}

/// The replay as text: how far it matched, and the first difference.
pub fn render_replay(replayed: &[Replayed]) -> String {
    match replayed.last() {
        None => "The run made no tool calls to replay.".to_string(),

        Some(last) if last.same() => format!(
            "All {} calls gave the same output as when they were recorded.",
            replayed.len()
        ),

        Some(last) => format!(
            "{} calls matched. Call {} (step {}, {}) differs.\n\nRecorded:\n{}\n\nNow:\n{}",
            replayed.len() - 1,
            replayed.len(),
            last.step,
            last.tool,
            clip(&last.recorded, 1200),
            clip(&last.now, 1200)
        ),
    }
}

fn clip(text: &str, max: usize) -> String {
    let text = text.trim();

    match text.char_indices().nth(max) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::message::ToolResult;
    use crate::usage::Usage;

    fn run(commands: &[(&str, bool)], outcome: Outcome) -> Vec<Event> {
        let mut events = vec![Event::RunStarted {
            instruction: "fix the build".to_string(),
            provider: "test".to_string(),
            model: "m".to_string(),
        }];

        for (index, (command, failed)) in commands.iter().enumerate() {
            let id = format!("{index}");

            events.push(Event::ToolStarted {
                step: index as u32 + 1,
                call: ToolCall {
                    id: id.clone(),
                    name: "shell".to_string(),
                    input: json!({ "command": command }),
                },
            });

            events.push(Event::ToolFinished {
                step: index as u32 + 1,
                tool: "shell".to_string(),
                result: ToolResult {
                    call_id: id,
                    output: format!("exit code: {}\n", if *failed { 1 } else { 0 }),
                    is_error: false,
                    images: Vec::new(),
                },
                duration_ms: 1,
            });
        }

        events.push(Event::RunFinished {
            outcome,
            steps: commands.len() as u32,
            usage: Usage {
                input_tokens: 10,
                output_tokens: 5,
                ..Usage::default()
            },
            duration_ms: 100,
            verified: None,
        });

        events
    }

    #[test]
    fn two_runs_are_compared_at_their_first_difference() {
        let passed = run(&[("ls", false), ("cargo test", false)], Outcome::Completed);
        let failed = run(&[("ls", false), ("npm test", true)], Outcome::StepLimit);

        let divergence = diverge(&actions(&passed), &actions(&failed));

        assert_eq!(divergence.shared, 1);
        assert!(divergence.left.unwrap().input.contains("cargo test"));
        assert!(divergence.right.unwrap().failed);
    }

    #[test]
    fn a_summary_counts_what_happened() {
        let events = run(&[("ls", false), ("npm test", true)], Outcome::Completed);
        let summary = summarize(&events);

        assert_eq!(summary.tools["shell"], 2);
        assert_eq!(summary.tokens, 15);
        assert_eq!(summary.outcome, Some(Outcome::Completed));
    }

    #[test]
    fn a_log_is_split_into_runs() {
        let mut events = run(&[("ls", false)], Outcome::Completed);
        events.extend(run(&[("pwd", false)], Outcome::Completed));

        assert_eq!(runs(&events).len(), 2);
    }
}
