//! Noticing that a run is going round in circles.
//!
//! A model that has failed at something tends to try it again, a little
//! differently or not at all. The [`StuckDetector`] watches the calls of a
//! run and, when the same call has failed several times or every call of
//! several steps in a row has failed, tells the model so once and asks it
//! to find out why before trying again.
//!
//! The note says what the failures look like: a missing program or a
//! refused permission is the environment, a call the tool rejected is how
//! it was called, failing tests are the task. Edits are compared loosely,
//! so the same patch with its spacing changed counts as the same, and an
//! edit that puts back what an earlier edit took out is pointed out.
//!
//! The agent asks for a [`Verdict`] after each step. Once the model has
//! been told and still fails, the run may move to a stronger model; if
//! failing goes on long after that, another attempt is unlikely to help
//! and the run is stopped.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use serde_json::{Value, json};

use crate::advice::{Advice, Adviser};
use crate::trajectory::{Event, TrajectorySink};

/// The same failing call this many times is a loop.
const SAME_FAILURE: u32 = 3;

/// Every call failing in this many steps in a row is a loop too.
const FAILING_STEPS: u32 = 4;

/// After being told, this many more failing steps in a row stop the run.
const STOP_AFTER_STEPS: u32 = 8;

/// After being told, the same call failing this many times stops the run.
const STOP_AFTER_SAME: u32 = 6;

const SOURCE: &str = "stuck";

/// What the agent should do about the run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Nothing out of the ordinary.
    Carry,

    /// The model was told it is going round in circles: a stronger model
    /// may get past it. Said once per run.
    Escalate(String),

    /// It went on failing after being told. Said once per run.
    Stop(String),
}

/// What a failure looks like it comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    /// A program, file, permission or connection the command needs.
    Environment,

    /// The call itself: wrong arguments, text to replace not found.
    Call,

    /// The work: failing tests, compiler errors.
    Task,

    Unknown,
}

impl Cause {
    fn hint(self) -> &'static str {
        match self {
            Cause::Environment => {
                "The errors point at the environment (a missing program or file, a permission, \
                 the network or the sandbox), not the code. Check what is installed and allowed \
                 (`which`, the package manager, the lockfile, versions) before another attempt."
            }

            Cause::Call => {
                "The errors point at how the tool is called: its input is rejected or the text \
                 to change is not where you think. Read the file or the tool's description again \
                 and call it with what is actually there."
            }

            Cause::Task => {
                "The errors come from the work itself (failing tests or a build error). Read the \
                 first error in full, find its cause in the code, and fix that rather than the \
                 symptom."
            }

            Cause::Unknown => {
                "Work out why it fails first: is the cause in the code, the command or its \
                 arguments, the environment, or the task itself?"
            }
        }
    }
}

#[derive(Default)]
struct State {
    /// The calls of the current run by id, as their signatures.
    calls: HashMap<String, String>,

    /// How often each signature has failed.
    failures: HashMap<String, u32>,

    /// Signatures already spoken about, so each is mentioned once.
    told: HashSet<String>,

    /// The step whose results are being counted, and how they went.
    step: u32,
    step_calls: u32,
    step_failures: u32,

    /// Steps in a row in which every call failed.
    failing_steps: u32,

    /// What to say after the current step, if anything.
    pending: Option<(String, Value)>,

    /// The cause of each failure so far.
    causes: Vec<Cause>,

    /// Per file, the text earlier edits replaced, loosely compared.
    replaced: HashMap<String, HashSet<String>>,

    /// Edits that put back what an earlier one took out.
    reversals: u32,

    /// The model has been told; what the agent was asked to do about it.
    escalation_due: Option<String>,
    escalated: bool,
    stopped: bool,

    /// Failing steps and the highest count of one failure when it was
    /// told, to measure what came after.
    told_at: Option<(u32, u32)>,
}

/// Watches a run for repeated failures. Add it to an agent both as a sink
/// and as an adviser.
#[derive(Default)]
pub struct StuckDetector {
    state: Mutex<State>,
}

impl StuckDetector {
    pub fn new() -> Self {
        Self::default()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// What to do about the run now. Asked after each step's advice.
    pub fn verdict(&self) -> Verdict {
        let mut state = self.state();

        if let Some(reason) = state.escalation_due.take()
            && !state.escalated
        {
            state.escalated = true;

            return Verdict::Escalate(reason);
        }

        let Some((steps_then, same_then)) = state.told_at else {
            return Verdict::Carry;
        };

        if state.stopped {
            return Verdict::Carry;
        }

        let worst = state.failures.values().copied().max().unwrap_or(0);

        let still_failing = state.failing_steps >= STOP_AFTER_STEPS.max(steps_then + 4)
            || (worst >= STOP_AFTER_SAME && worst >= same_then + 3);

        if !still_failing {
            return Verdict::Carry;
        }

        state.stopped = true;

        Verdict::Stop(format!(
            "The run kept failing after it was told to change approach ({} failing steps in a \
             row, one call failed {worst} times). {}",
            state.failing_steps,
            state.main_cause().hint()
        ))
    }
}

impl TrajectorySink for StuckDetector {
    fn record(&self, event: &Event) {
        let mut state = self.state();

        match event {
            Event::RunStarted { .. } => *state = State::default(),

            Event::ToolStarted { step, call } => {
                if *step != state.step {
                    state.close_step();
                    state.step = *step;
                }

                state.step_calls += 1;
                state
                    .calls
                    .insert(call.id.clone(), signature(&call.name, &call.input));

                state.watch_edit(&call.name, &call.input);
            }

            Event::ToolFinished { tool, result, .. } => {
                if !failed(tool, &result.output, result.is_error) {
                    return;
                }

                state.step_failures += 1;
                state
                    .causes
                    .push(cause(tool, &result.output, result.is_error));

                let Some(signature) = state.calls.get(&result.call_id).cloned() else {
                    return;
                };

                let count = state.failures.entry(signature.clone()).or_default();
                *count += 1;
                let count = *count;

                if count >= SAME_FAILURE && state.told.insert(signature.clone()) {
                    let cause = state.main_cause();

                    state.pending = Some((
                        format!(
                            "The same `{tool}` call has now failed {count} times with the same \
                             input. Do not run it again as it is. {} Check that assumption and \
                             change approach. If it cannot be done here, say what blocks it.",
                            cause.hint()
                        ),
                        json!({ "kind": "same_failure", "tool": tool, "count": count, "cause": format!("{cause:?}") }),
                    ));
                }
            }

            _ => {}
        }
    }
}

impl Adviser for StuckDetector {
    fn begin_run(&self, _max_steps: u32) {
        *self.state() = State::default();
    }

    fn advise(&self, _step: u32, _max_steps: u32) -> Option<Advice> {
        let mut state = self.state();

        state.close_step();

        if state.pending.is_none() && state.reversals >= 2 && state.told.insert("reversals".into())
        {
            state.pending = Some((
                "You are undoing your own edits: changes you made earlier have been put back \
                 more than once. Stop editing and decide which version is right, and why, \
                 before the next change."
                    .to_string(),
                json!({ "kind": "reversals", "count": state.reversals }),
            ));
        }

        if state.pending.is_none()
            && state.failing_steps >= FAILING_STEPS
            && state.told.insert("failing_steps".to_string())
        {
            let steps = state.failing_steps;

            state.pending = Some((
                format!(
                    "Every tool call in the last {steps} steps has failed. Stop and take stock \
                     before the next action: list what you have tried, what each failure said, \
                     and what they have in common. {} Then choose a different approach, or \
                     report that the task is blocked and why.",
                    state.main_cause().hint()
                ),
                json!({ "kind": "failing_steps", "steps": steps }),
            ));
        }

        let (text, detail) = state.pending.take()?;

        if state.told_at.is_none() {
            let worst = state.failures.values().copied().max().unwrap_or(0);

            state.told_at = Some((state.failing_steps, worst));
            state.escalation_due = Some(first_line(&text));
        }

        Some(Advice {
            source: SOURCE.to_string(),
            text,
            detail,
        })
    }
}

impl State {
    /// The most common cause of the failures so far.
    fn main_cause(&self) -> Cause {
        let order = [Cause::Environment, Cause::Call, Cause::Task];

        order
            .into_iter()
            .map(|cause| {
                (
                    self.causes.iter().filter(|seen| **seen == cause).count(),
                    cause,
                )
            })
            .filter(|(count, _)| *count > 0)
            .max_by_key(|(count, _)| *count)
            .map_or(Cause::Unknown, |(_, cause)| cause)
    }

    /// Notes what an edit replaces, and whether it puts back what an
    /// earlier edit of the same file took out.
    fn watch_edit(&mut self, tool: &str, input: &Value) {
        if tool != "edit_file" {
            return;
        }

        let Some(path) = input.get("path").and_then(Value::as_str) else {
            return;
        };

        let text = |key: &str| input.get(key).and_then(Value::as_str).map(loose);

        let (Some(old), Some(new)) = (text("old_string"), text("new_string")) else {
            return;
        };

        let replaced = self.replaced.entry(path.to_string()).or_default();

        if !new.is_empty() && replaced.contains(&new) {
            self.reversals += 1;
        }

        replaced.insert(old);
    }

    /// Counts the step just ended towards the run of failing steps.
    fn close_step(&mut self) {
        if self.step_calls == 0 {
            return;
        }

        if self.step_failures == self.step_calls {
            self.failing_steps += 1;
        } else {
            self.failing_steps = 0;
        }

        self.step_calls = 0;
        self.step_failures = 0;
    }
}

/// A call's tool and input, the same however the input's keys are
/// ordered. Edits are compared loosely, so a patch retried with its
/// spacing changed is the same patch.
fn signature(tool: &str, input: &Value) -> String {
    if matches!(tool, "edit_file" | "write_file") {
        let field = |key: &str| input.get(key).and_then(Value::as_str).map(loose);

        return format!(
            "{tool} {} {}",
            field("path").unwrap_or_default(),
            field("old_string").unwrap_or_default()
        );
    }

    // `serde_json` keeps object keys sorted unless told otherwise, so the
    // text of a value is already canonical.
    format!("{tool} {input}")
}

/// Text with its runs of white space made one space.
fn loose(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn first_line(text: &str) -> String {
    text.split(". ").next().unwrap_or(text).to_string()
}

/// What a failure's output says it comes from.
pub fn cause(tool: &str, output: &str, is_error: bool) -> Cause {
    let text = output.to_lowercase();

    let any = |needles: &[&str]| needles.iter().any(|needle| text.contains(needle));

    if any(&[
        "command not found",
        "not recognized as",
        "permission denied",
        "operation not permitted",
        "could not resolve host",
        "connection refused",
        "network is unreachable",
        "enotfound",
        "eacces",
        "no such file or directory",
        "not installed",
        "sandbox",
        "timed out",
    ]) {
        return Cause::Environment;
    }

    if is_error && tool != "shell" {
        return Cause::Call;
    }

    if any(&[
        "test result: failed",
        "failed",
        "assertion",
        "error[e",
        "error:",
        "traceback",
        "exception",
    ]) {
        return Cause::Task;
    }

    Cause::Unknown
}

/// Whether a call failed. A command that ran and exited non-zero is a
/// failure here, though the shell tool reports it as output.
pub(crate) fn failed(tool: &str, output: &str, is_error: bool) -> bool {
    if is_error {
        return true;
    }

    tool == "shell"
        && output
            .strip_prefix("exit code: ")
            .is_some_and(|rest| !rest.starts_with("0\n") && rest != "0")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{ToolCall, ToolResult};

    fn call(detector: &StuckDetector, step: u32, id: &str, command: &str, exit: i32) {
        detector.record(&Event::ToolStarted {
            step,
            call: ToolCall {
                id: id.to_string(),
                name: "shell".to_string(),
                input: json!({ "command": command }),
            },
        });

        detector.record(&Event::ToolFinished {
            step,
            tool: "shell".to_string(),
            result: ToolResult {
                call_id: id.to_string(),
                output: format!("exit code: {exit}\n\nstderr:\nboom"),
                is_error: false,
                images: Vec::new(),
            },
            duration_ms: 1,
        });
    }

    #[test]
    fn the_third_identical_failure_is_pointed_out_once() {
        let detector = StuckDetector::new();

        call(&detector, 1, "a", "npm install", 1);
        assert!(detector.advise(1, 50).is_none());

        call(&detector, 2, "b", "npm install", 1);
        assert!(detector.advise(2, 50).is_none());

        call(&detector, 3, "c", "npm install", 1);
        let advice = detector
            .advise(3, 50)
            .expect("advice after the third failure");
        assert!(advice.text.contains("3 times"));

        call(&detector, 4, "d", "npm install", 1);
        assert!(
            detector
                .advise(4, 50)
                .is_none_or(|advice| !advice.text.contains("same"))
        );
    }

    #[test]
    fn different_failures_in_a_row_are_noticed() {
        let detector = StuckDetector::new();

        for step in 1..=4 {
            call(
                &detector,
                step,
                &format!("{step}"),
                &format!("try {step}"),
                2,
            );

            let advice = detector.advise(step, 50);

            assert_eq!(advice.is_some(), step == 4, "step {step}");
        }
    }

    #[test]
    fn failures_that_go_on_after_the_advice_escalate_then_stop() {
        let detector = StuckDetector::new();

        for step in 1..=3 {
            call(&detector, step, &format!("{step}"), "npm install", 1);
            detector.advise(step, 50);
        }

        assert!(matches!(detector.verdict(), Verdict::Escalate(_)));
        assert_eq!(detector.verdict(), Verdict::Carry);

        let mut stopped = false;

        for step in 4..=12 {
            call(&detector, step, &format!("{step}"), "npm install", 1);
            detector.advise(step, 50);

            if let Verdict::Stop(_) = detector.verdict() {
                stopped = true;
                break;
            }
        }

        assert!(stopped);
    }

    #[test]
    fn causes_are_told_apart() {
        assert_eq!(
            cause("shell", "exit code: 127\nnpm: command not found", false),
            Cause::Environment
        );
        assert_eq!(
            cause("edit_file", "old_string not found", true),
            Cause::Call
        );
        assert_eq!(
            cause(
                "shell",
                "exit code: 101\ntest result: FAILED. 3 passed",
                false
            ),
            Cause::Task
        );
    }

    #[test]
    fn edits_that_undo_each_other_are_noticed() {
        let detector = StuckDetector::new();

        let edit = |step: u32, old: &str, new: &str| {
            detector.record(&Event::ToolStarted {
                step,
                call: ToolCall {
                    id: format!("{step}"),
                    name: "edit_file".to_string(),
                    input: json!({ "path": "a.rs", "old_string": old, "new_string": new }),
                },
            });
        };

        edit(1, "let x = 1;", "let x = 2;");
        edit(2, "let x = 2;", "let  x = 1;");
        edit(3, "let x = 1;", "let x = 2;");

        let advice = detector.advise(3, 50).expect("advice about the reversals");
        assert!(advice.text.contains("undoing"));
    }

    #[test]
    fn a_success_breaks_the_run_of_failures() {
        let detector = StuckDetector::new();

        for step in 1..=6 {
            let exit = if step == 3 { 0 } else { 1 };

            call(
                &detector,
                step,
                &format!("{step}"),
                &format!("try {step}"),
                exit,
            );

            assert!(detector.advise(step, 50).is_none(), "step {step}");
        }
    }
}
