//! What a controller can see of a run: where it stands, and what the model
//! did at each step.

use std::collections::HashMap;

use cortex_core::{Event, Message, StopReason, ToolCall};
use serde_json::Value;

/// How many numbers describe a situation.
pub const FEATURES: usize = 17;
pub const FEATURE_NAMES: [&str; FEATURES] = [
    "bias",
    "step budget used",
    "has edits",
    "edits unchecked",
    "has commands",
    "last command failed",
    "consecutive failures",
    "reads since edit",
    "read count",
    "edit count",
    "gate rejected",
    "tool errored",
    "last inspect",
    "last edit",
    "last run",
    "last revert",
    "repeated action",
];

/// The kinds of step a coding agent can take next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Read or search without changing anything.
    Inspect,

    /// Change a file.
    Edit,

    /// Run a command: tests, a build, the program.
    Run,

    /// Undo changes through version control.
    Revert,

    /// Stop and report.
    Finish,
}

impl Action {
    pub const ALL: [Self; 5] = [
        Self::Inspect,
        Self::Edit,
        Self::Run,
        Self::Revert,
        Self::Finish,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Inspect => "inspect",
            Self::Edit => "edit",
            Self::Run => "run",
            Self::Revert => "revert",
            Self::Finish => "finish",
        }
    }

    /// The action as it reads in a sentence of advice.
    pub fn phrase(self) -> &'static str {
        match self {
            Self::Inspect => "read or search more code",
            Self::Edit => "edit the code",
            Self::Run => "run a command to check the work",
            Self::Revert => "undo the last change",
            Self::Finish => "finish and report",
        }
    }

    /// The action a model response amounts to. A response that calls
    /// several tools counts as the most consequential of them.
    pub fn of_response(message: &Message, stop_reason: &StopReason) -> Self {
        let calls = message.tool_calls();

        if calls.is_empty() || *stop_reason != StopReason::ToolUse {
            return Self::Finish;
        }

        let kinds: Vec<Self> = calls.iter().map(Self::of_call).collect();

        [Self::Revert, Self::Edit, Self::Run]
            .into_iter()
            .find(|kind| kinds.contains(kind))
            .unwrap_or(Self::Inspect)
    }

    fn of_call(call: &ToolCall) -> Self {
        match call.name.as_str() {
            "edit_file" | "write_file" => Self::Edit,

            "shell" => {
                let command = call
                    .input
                    .get("command")
                    .and_then(Value::as_str)
                    .unwrap_or_default();

                let reverts = ["git checkout", "git restore", "git stash", "git reset"];

                if reverts.iter().any(|revert| command.contains(revert)) {
                    Self::Revert
                } else {
                    Self::Run
                }
            }

            _ => Self::Inspect,
        }
    }
}

/// The state of the current run, kept up to date from its events.
#[derive(Debug, Clone, Default)]
pub struct Situation {
    reads: u32,
    reads_since_edit: u32,
    edits: u32,
    runs: u32,

    /// A file was changed and nothing has been run since.
    edited_since_run: bool,

    last_run_failed: bool,

    /// Commands and tool calls that failed one after another.
    failures_in_a_row: u32,

    last_tool_errored: bool,
    gate_rejected: bool,

    last_action: Option<Action>,

    /// How many steps in a row took `last_action`.
    repeats: u32,

    pending: HashMap<String, ToolCall>,
}

impl Situation {
    pub fn last_action(&self) -> Option<Action> {
        self.last_action
    }

    /// Steps in a row spent on the same kind of action while things kept
    /// failing: the sign of a loop.
    pub fn stuck_repeats(&self) -> u32 {
        if self.failures_in_a_row >= 2 {
            self.repeats
        } else {
            0
        }
    }

    /// Records the action the model chose for a step.
    pub fn note_action(&mut self, action: Action) {
        if self.last_action == Some(action) {
            self.repeats += 1;
        } else {
            self.repeats = 1;
        }

        self.last_action = Some(action);
    }

    pub fn observe(&mut self, event: &Event) {
        match event {
            Event::RunStarted { .. } => *self = Self::default(),

            Event::ToolStarted { call, .. } => {
                self.pending.insert(call.id.clone(), call.clone());
            }

            Event::ToolFinished { result, .. } => {
                let Some(call) = self.pending.remove(&result.call_id) else {
                    return;
                };

                self.last_tool_errored = result.is_error;

                let failed = match Action::of_call(&call) {
                    Action::Edit => {
                        if !result.is_error {
                            self.edits += 1;
                            self.reads_since_edit = 0;
                            self.edited_since_run = true;
                        }

                        result.is_error
                    }

                    Action::Run | Action::Revert => {
                        let failed = result.is_error || !result.output.starts_with("exit code: 0");

                        self.runs += 1;
                        self.edited_since_run = false;
                        self.last_run_failed = failed;

                        failed
                    }

                    Action::Inspect | Action::Finish => {
                        self.reads += 1;
                        self.reads_since_edit += 1;

                        result.is_error
                    }
                };

                if failed {
                    self.failures_in_a_row += 1;
                } else {
                    self.failures_in_a_row = 0;
                }
            }

            Event::VerificationRan { passed, .. } => {
                self.gate_rejected = !passed;
                self.edited_since_run = false;
                self.last_run_failed = !passed;
            }

            _ => {}
        }
    }

    /// The situation as numbers between 0 and 1.
    pub fn features(&self, step: u32, max_steps: u32) -> [f32; FEATURES] {
        let flag = |condition: bool| if condition { 1.0 } else { 0.0 };

        let scaled = |count: u32, full: u32| (count as f32 / full as f32).min(1.0);

        let last = |action: Action| flag(self.last_action == Some(action));

        [
            1.0,
            scaled(step, max_steps.max(1)),
            flag(self.edits > 0),
            flag(self.edited_since_run),
            flag(self.runs > 0),
            flag(self.last_run_failed),
            scaled(self.failures_in_a_row, 3),
            scaled(self.reads_since_edit, 5),
            scaled(self.reads, 10),
            scaled(self.edits, 5),
            flag(self.gate_rejected),
            flag(self.last_tool_errored),
            last(Action::Inspect),
            last(Action::Edit),
            last(Action::Run),
            last(Action::Revert),
            scaled(self.repeats, 4),
        ]
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use cortex_core::{Block, Role, ToolResult};
    use serde_json::json;

    use super::*;

    pub(crate) fn response(calls: &[(&str, Value)]) -> (Message, StopReason) {
        let blocks = calls
            .iter()
            .enumerate()
            .map(|(index, (name, input))| {
                Block::ToolCall(ToolCall {
                    id: index.to_string(),
                    name: name.to_string(),
                    input: input.clone(),
                })
            })
            .collect();

        let stop_reason = if calls.is_empty() {
            StopReason::EndTurn
        } else {
            StopReason::ToolUse
        };

        (
            Message {
                role: Role::Assistant,
                blocks,
            },
            stop_reason,
        )
    }

    pub(crate) fn tool(situation: &mut Situation, name: &str, input: Value, output: &str) {
        let events = tool_events(name, input, output);

        situation.observe(&events[0]);
        situation.observe(&events[1]);
    }

    pub(crate) fn tool_events(name: &str, input: Value, output: &str) -> [Event; 2] {
        let started = Event::ToolStarted {
            step: 1,
            call: ToolCall {
                id: "call".to_string(),
                name: name.to_string(),
                input,
            },
        };

        let finished = Event::ToolFinished {
            step: 1,
            tool: name.to_string(),
            result: ToolResult {
                call_id: "call".to_string(),
                output: output.to_string(),
                is_error: false,
                images: Vec::new(),
            },
            duration_ms: 1,
        };

        [started, finished]
    }

    #[test]
    fn a_response_is_its_most_consequential_call() {
        let action = |calls: &[(&str, Value)]| {
            let (message, stop_reason) = response(calls);

            Action::of_response(&message, &stop_reason)
        };

        assert_eq!(action(&[]), Action::Finish);
        assert_eq!(action(&[("read_file", json!({}))]), Action::Inspect);

        assert_eq!(
            action(&[("read_file", json!({})), ("edit_file", json!({}))]),
            Action::Edit
        );

        assert_eq!(
            action(&[("shell", json!({ "command": "npm test" }))]),
            Action::Run
        );

        assert_eq!(
            action(&[("shell", json!({ "command": "git checkout -- a.ts" }))]),
            Action::Revert
        );
    }

    #[test]
    fn features_follow_the_run() {
        let mut situation = Situation::default();

        tool(&mut situation, "read_file", json!({}), "...");
        tool(&mut situation, "edit_file", json!({}), "edited");

        let edited = situation.features(2, 10);

        // Edited, and not yet run.
        assert_eq!(edited[2], 1.0);
        assert_eq!(edited[3], 1.0);
        assert_eq!(edited[4], 0.0);

        tool(
            &mut situation,
            "shell",
            json!({ "command": "npm test" }),
            "exit code: 1",
        );

        let ran = situation.features(3, 10);

        assert_eq!(ran[3], 0.0);
        assert_eq!(ran[4], 1.0);
        assert_eq!(ran[5], 1.0);
        assert!(ran.iter().all(|value| (0.0..=1.0).contains(value)));
    }
}
