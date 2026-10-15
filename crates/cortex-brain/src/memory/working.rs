//! Working memory: what the current run has done so far.
//!
//! It is built by watching the run's events, so it costs the model nothing.
//! It lasts for one task. When the task ends it is consolidated into an
//! episode; during the task the agent can ask for it with the `recall`
//! tool, which matters once older tool output has been trimmed from the
//! conversation.

use std::collections::HashMap;

use cortex_core::{CheckOutcome, Event, Outcome, ToolCall, ToolResult};
use serde_json::Value;

/// Tools whose `path` input is a file being read.
const READING_TOOLS: [&str; 4] = ["read_file", "outline", "read_symbol", "file_dependencies"];

/// Tools whose `path` input is a file being changed.
const WRITING_TOOLS: [&str; 2] = ["edit_file", "write_file"];

const SEARCH_TOOLS: [&str; 3] = ["search", "semantic_search", "find_references"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRecord {
    pub command: String,
    pub succeeded: bool,
}

#[derive(Debug, Clone, Default)]
pub struct WorkingMemory {
    pub instruction: String,

    /// Files looked at, in the order they were first opened.
    pub files_read: Vec<String>,

    /// Files edited or written successfully.
    pub files_changed: Vec<String>,

    pub searches: Vec<String>,
    pub commands: Vec<CommandRecord>,
    pub calculations: Vec<String>,

    /// Tool calls that returned an error, as `tool: first line`.
    pub failures: Vec<String>,

    /// Facts the agent chose to remember during this run.
    pub notes: Vec<String>,

    /// The model's most recent message with text; at the end of a run this
    /// is its report.
    pub last_text: String,

    /// The checks from the most recent run of the verification gate.
    pub checks: Vec<CheckOutcome>,

    /// Times the gate sent the work back.
    pub rejections: u32,

    pub verified: Option<bool>,
    pub outcome: Option<Outcome>,
    pub steps: u32,
    pub tokens: u64,
    pub duration_ms: u64,

    /// Calls that have started and not yet finished, by call id.
    pub(crate) pending: HashMap<String, ToolCall>,
}

impl WorkingMemory {
    /// Updates the memory with one event of the run.
    pub fn observe(&mut self, event: &Event) {
        match event {
            Event::UserInterjected { text, .. } => {
                self.instruction.push_str("\nFollow-up: ");
                self.instruction.push_str(text);
            }
            Event::BrainObserved { .. } | Event::ImagesAttached { .. } => {}
            Event::RunStarted { instruction, .. } => {
                *self = Self {
                    instruction: instruction.clone(),
                    ..Self::default()
                };
            }

            Event::ModelResponded { message, .. } => {
                let text = message.text();

                if !text.trim().is_empty() {
                    self.last_text = text.trim().to_string();
                }
            }

            Event::ToolStarted { call, .. } => {
                self.pending.insert(call.id.clone(), call.clone());
            }

            Event::ToolFinished { result, .. } => {
                if let Some(call) = self.pending.remove(&result.call_id) {
                    self.observe_tool(&call, result);
                }
            }

            Event::VerificationRan { passed, checks, .. } => {
                self.checks = checks.clone();
                self.verified = Some(*passed);

                if !passed {
                    self.rejections += 1;
                }
            }

            Event::RunFinished {
                outcome,
                steps,
                usage,
                duration_ms,
                verified,
            } => {
                self.outcome = Some(*outcome);
                self.steps = *steps;
                self.tokens = usage.total();
                self.duration_ms = *duration_ms;
                self.verified = *verified;
            }

            Event::ContextPrepared { .. }
            | Event::ContextPruned { .. }
            | Event::ContextMasked { .. }
            | Event::ContextCompacted { .. }
            | Event::ContextMeasured { .. }
            | Event::ExperimentAssigned { .. }
            | Event::ContextWithheld { .. }
            | Event::ReviewRan { .. }
            | Event::LessonsLearned { .. }
            | Event::AttemptStarted { .. }
            | Event::AttemptFinished { .. }
            | Event::AttemptChosen { .. }
            | Event::ModelStarted { .. }
            | Event::RunResumed { .. } => {}
        }
    }

    fn observe_tool(&mut self, call: &ToolCall, result: &ToolResult) {
        let name = call.name.as_str();

        if result.is_error {
            let first_line = result.output.lines().next().unwrap_or_default();

            push_unique(&mut self.failures, format!("{name}: {first_line}"));

            return;
        }

        let text = |field: &str| call.input.get(field).and_then(Value::as_str);
        if name == "calculate" {
            push_unique(
                &mut self.calculations,
                result.output.chars().take(200).collect(),
            );
        }

        if READING_TOOLS.contains(&name)
            && let Some(path) = text("path")
        {
            push_unique(&mut self.files_read, path.to_string());
        }

        if WRITING_TOOLS.contains(&name)
            && let Some(path) = text("path")
        {
            push_unique(&mut self.files_changed, path.to_string());
        }

        if SEARCH_TOOLS.contains(&name)
            && let Some(query) = text("pattern").or(text("query")).or(text("name"))
        {
            push_unique(&mut self.searches, query.to_string());
        }

        if name == "shell"
            && let Some(command) = text("command")
        {
            self.commands.push(CommandRecord {
                command: command.to_string(),
                succeeded: result.output.starts_with("exit code: 0"),
            });
        }
    }

    pub fn note(&mut self, text: &str) {
        push_unique(&mut self.notes, text.to_string());
    }

    /// A short account of the run so far, for the `recall` tool.
    pub fn digest(&self) -> String {
        let mut text = String::from("This task so far:\n");

        let mut line = |label: &str, items: &[String]| {
            if !items.is_empty() {
                text.push_str(&format!("- {label}: {}\n", items.join(", ")));
            }
        };

        line("Files changed", &self.files_changed);
        line("Files read", &self.files_read);
        line("Searches", &self.searches);

        let failed: Vec<String> = self
            .commands
            .iter()
            .filter(|record| !record.succeeded)
            .map(|record| format!("`{}`", first_line(&record.command)))
            .collect();

        line("Commands that failed", &failed);
        line("Tool errors", &self.failures);
        line("Notes", &self.notes);

        if self.rejections > 0 {
            text.push_str(&format!(
                "- The project's checks have rejected the work {} time(s).\n",
                self.rejections
            ));
        }

        for calculation in self.calculations.iter().rev().take(5) {
            text.push_str(&format!("- Calculated: {calculation}\n"));
        }

        if text.lines().count() == 1 {
            text.push_str("- Nothing has been read, changed or run yet.\n");
        }

        text
    }
}

fn push_unique(list: &mut Vec<String>, item: String) {
    if !list.contains(&item) {
        list.push(item);
    }
}

pub fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or_default().trim()
}

#[cfg(test)]
mod tests {
    use cortex_core::Usage;
    use serde_json::json;

    use super::*;

    /// Feeds one finished tool call into `memory`.
    pub(crate) fn run_tool(
        memory: &mut WorkingMemory,
        id: &str,
        tool: &str,
        input: Value,
        output: &str,
        is_error: bool,
    ) {
        memory.observe(&Event::ToolStarted {
            step: 1,
            call: ToolCall {
                id: id.to_string(),
                name: tool.to_string(),
                input,
            },
        });

        memory.observe(&Event::ToolFinished {
            step: 1,
            tool: tool.to_string(),
            result: ToolResult {
                call_id: id.to_string(),
                output: output.to_string(),
                is_error,
                images: Vec::new(),
            },
            duration_ms: 1,
        });
    }

    #[test]
    fn a_run_is_summarised_from_its_events() {
        let mut memory = WorkingMemory::default();

        memory.observe(&Event::RunStarted {
            instruction: "fix the calendar".to_string(),
            provider: "scripted".to_string(),
            model: "scripted".to_string(),
        });

        run_tool(
            &mut memory,
            "1",
            "read_file",
            json!({ "path": "a.ts" }),
            "...",
            false,
        );
        run_tool(
            &mut memory,
            "2",
            "read_file",
            json!({ "path": "a.ts" }),
            "...",
            false,
        );
        run_tool(
            &mut memory,
            "3",
            "edit_file",
            json!({ "path": "a.ts" }),
            "ok",
            false,
        );
        run_tool(
            &mut memory,
            "4",
            "edit_file",
            json!({ "path": "b.ts" }),
            "no match",
            true,
        );
        run_tool(
            &mut memory,
            "5",
            "shell",
            json!({ "command": "npx jest" }),
            "exit code: 1",
            false,
        );
        run_tool(
            &mut memory,
            "6",
            "shell",
            json!({ "command": "npm test" }),
            "exit code: 0",
            false,
        );

        memory.observe(&Event::RunFinished {
            outcome: Outcome::Completed,
            steps: 6,
            usage: Usage::default(),
            duration_ms: 10,
            verified: Some(true),
        });

        assert_eq!(memory.instruction, "fix the calendar");
        assert_eq!(memory.files_read, ["a.ts"]);
        assert_eq!(memory.files_changed, ["a.ts"]);
        assert_eq!(memory.failures, ["edit_file: no match"]);
        assert_eq!(memory.commands.len(), 2);
        assert!(!memory.commands[0].succeeded);
        assert!(memory.commands[1].succeeded);
        assert_eq!(memory.verified, Some(true));

        let digest = memory.digest();

        assert!(digest.contains("Files changed: a.ts"));
        assert!(digest.contains("`npx jest`"));
    }

    #[test]
    fn a_new_run_starts_empty() {
        let mut memory = WorkingMemory::default();

        run_tool(
            &mut memory,
            "1",
            "read_file",
            json!({ "path": "a.ts" }),
            "...",
            false,
        );

        memory.observe(&Event::RunStarted {
            instruction: "next".to_string(),
            provider: "scripted".to_string(),
            model: "scripted".to_string(),
        });

        assert!(memory.files_read.is_empty());
    }
}
