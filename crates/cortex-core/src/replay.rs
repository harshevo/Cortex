//! Rebuilding a conversation from its trajectory log.
//!
//! A session's log records every message that went to and came from the
//! model. Reading it back gives the conversation as the model last saw
//! it, so a session that was closed can be taken up again where it was
//! left.
//!
//! One difference is allowed: tool output that was pruned during the run
//! is back in full, since the log keeps what the tools returned.

use crate::briefing::{self, Briefing};
use crate::inbox;
use crate::message::{Block, Message, Role, ToolResult};
use crate::provider::StopReason;
use crate::trajectory::Event;
use crate::verify::Verification;

/// The conversation that `events`, in order, add up to.
pub fn conversation(events: &[Event]) -> Vec<Message> {
    let mut replay = Replay::default();

    for event in events {
        replay.apply(event);
    }

    replay.settle();
    replay.conversation
}

#[derive(Default)]
struct Replay {
    conversation: Vec<Message>,

    /// An instruction whose first model call has not been seen yet, and
    /// the context gathered for it.
    instruction: Option<String>,
    briefings: Vec<Briefing>,

    /// What is being sent back after a response that called tools.
    answer: Option<Vec<Block>>,

    /// What the gate said about work that did not pass. It was only sent
    /// if the model was asked again.
    feedback: Option<String>,
}

impl Replay {
    fn apply(&mut self, event: &Event) {
        match event {
            Event::BrainObserved { .. } => {}
            Event::RunStarted { instruction, .. } => {
                self.settle();

                self.instruction = Some(instruction.clone());
                self.briefings.clear();
            }

            Event::ContextPrepared {
                source,
                text,
                detail,
                ..
            } => {
                // Before the first model call it is a briefing; after a
                // step's tools it is advice.
                if self.instruction.is_some() {
                    self.briefings.push(Briefing {
                        source: source.clone(),
                        text: text.clone(),
                        detail: detail.clone(),
                    });
                } else if let Some(answer) = &mut self.answer {
                    answer.push(Block::Text { text: text.clone() });
                }
            }

            Event::ModelResponded {
                message,
                stop_reason,
                ..
            } => {
                self.open_task();
                self.send_answer();

                if let Some(feedback) = self.feedback.take() {
                    self.conversation.push(Message::user_text(feedback));
                }

                self.conversation.push(message.clone());

                if *stop_reason == StopReason::ToolUse {
                    self.answer = Some(Vec::new());
                }
            }

            Event::ToolFinished { result, .. } => {
                if let Some(answer) = &mut self.answer {
                    answer.push(Block::ToolResult(result.clone()));
                }
            }

            Event::UserInterjected { text, .. } => {
                let block = Block::Text {
                    text: inbox::as_shown(text),
                };

                match &mut self.answer {
                    Some(answer) => answer.push(block),

                    None => self.conversation.push(Message {
                        role: Role::User,
                        blocks: vec![block],
                    }),
                }
            }

            Event::VerificationRan { passed, checks, .. } => {
                self.feedback = (!passed).then(|| {
                    Verification {
                        checks: checks.clone(),
                    }
                    .feedback()
                });
            }

            // Only sent when the checks had passed, so it never has to
            // share the slot with their feedback.
            Event::ReviewRan { feedback, .. } => {
                if feedback.is_some() {
                    self.feedback = feedback.clone();
                }
            }

            Event::RunFinished { .. } => self.settle(),

            Event::RunResumed { .. }
            | Event::ModelStarted { .. }
            | Event::ToolStarted { .. }
            | Event::ContextPruned { .. }
            | Event::ContextMasked { .. }
            | Event::LessonsLearned { .. }
            | Event::AttemptStarted { .. }
            | Event::AttemptFinished { .. }
            | Event::AttemptChosen { .. } => {}
        }
    }

    /// Puts the instruction, with its briefings, into the conversation.
    fn open_task(&mut self) {
        if let Some(instruction) = self.instruction.take() {
            let text = briefing::compose(&self.briefings, &instruction);

            self.conversation.push(Message::user_text(text));
        }
    }

    /// Sends back the results of the last response's calls. A call whose
    /// result never reached the log (the run stopped first) is answered
    /// as not run, because a provider rejects a call left unanswered.
    fn send_answer(&mut self) {
        let Some(mut answer) = self.answer.take() else {
            return;
        };

        let calls = self
            .conversation
            .last()
            .map(Message::tool_calls)
            .unwrap_or_default();

        let answered = |id: &str, answer: &[Block]| {
            answer
                .iter()
                .any(|block| matches!(block, Block::ToolResult(result) if result.call_id == id))
        };

        let missing: Vec<Block> = calls
            .into_iter()
            .filter(|call| !answered(&call.id, &answer))
            .map(|call| {
                Block::ToolResult(ToolResult {
                    call_id: call.id,
                    output: "This call was not run because the run stopped first.".to_string(),
                    is_error: true,
                })
            })
            .collect();

        // Results come before any note added after them.
        answer.splice(0..0, missing);

        if !answer.is_empty() {
            self.conversation.push(Message {
                role: Role::User,
                blocks: answer,
            });
        }
    }

    /// Ends whatever run was in progress.
    fn settle(&mut self) {
        // An instruction no model call followed was never answered; it is
        // kept, so the conversation shows it was asked.
        self.open_task();
        self.send_answer();

        self.feedback = None;
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::agent::Outcome;
    use crate::message::ToolCall;
    use crate::usage::Usage;

    fn started(instruction: &str) -> Event {
        Event::RunStarted {
            instruction: instruction.to_string(),
            provider: "scripted".to_string(),
            model: "scripted".to_string(),
        }
    }

    fn calls_tool(step: u32, id: &str) -> Event {
        Event::ModelResponded {
            step,
            message: Message {
                role: Role::Assistant,
                blocks: vec![Block::ToolCall(ToolCall {
                    id: id.to_string(),
                    name: "read_file".to_string(),
                    input: json!({ "path": "a.rs" }),
                })],
            },
            stop_reason: StopReason::ToolUse,
            usage: Usage::default(),
            duration_ms: 1,
        }
    }

    fn says(step: u32, text: &str) -> Event {
        Event::ModelResponded {
            step,
            message: Message::assistant_text(text),
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
            duration_ms: 1,
        }
    }

    fn tool_finished(id: &str) -> Event {
        Event::ToolFinished {
            step: 1,
            tool: "read_file".to_string(),
            result: ToolResult {
                call_id: id.to_string(),
                output: "contents".to_string(),
                is_error: false,
            },
            duration_ms: 1,
        }
    }

    fn finished() -> Event {
        Event::RunFinished {
            outcome: Outcome::Completed,
            steps: 2,
            usage: Usage::default(),
            duration_ms: 1,
            verified: None,
        }
    }

    fn roles(conversation: &[Message]) -> Vec<Role> {
        conversation.iter().map(|message| message.role).collect()
    }

    #[test]
    fn two_runs_are_one_conversation() {
        let events = [
            started("read a.rs"),
            Event::ContextPrepared {
                source: "memory".to_string(),
                text: "## Memory\n\nNothing yet.".to_string(),
                detail: json!({}),
                duration_ms: 1,
            },
            calls_tool(1, "one"),
            tool_finished("one"),
            Event::ContextPrepared {
                source: "controller".to_string(),
                text: "A hint.".to_string(),
                detail: json!({}),
                duration_ms: 1,
            },
            says(2, "Done."),
            finished(),
            started("and now b.rs"),
            says(1, "Done again."),
            finished(),
        ];

        let conversation = conversation(&events);

        assert_eq!(
            roles(&conversation),
            [
                Role::User,
                Role::Assistant,
                Role::User,
                Role::Assistant,
                Role::User,
                Role::Assistant
            ]
        );

        // The briefing is in front of the task.
        assert!(conversation[0].text().starts_with("## Memory"));
        assert!(conversation[0].text().ends_with("read a.rs"));

        // The advice follows the tool result it was given with.
        assert!(matches!(conversation[2].blocks[0], Block::ToolResult(_)));
        assert_eq!(conversation[2].text(), "A hint.");

        assert_eq!(conversation[4].text(), "and now b.rs");
    }

    #[test]
    fn a_run_that_stopped_mid_step_can_still_be_sent() {
        let events = [started("read a.rs"), calls_tool(1, "one")];

        let conversation = conversation(&events);

        assert_eq!(
            roles(&conversation),
            [Role::User, Role::Assistant, Role::User]
        );

        assert!(matches!(
            &conversation[2].blocks[0],
            Block::ToolResult(result) if result.call_id == "one" && result.is_error
        ));
    }

    #[test]
    fn what_the_person_said_mid_run_is_where_they_said_it() {
        let events = [
            started("read a.rs"),
            calls_tool(1, "one"),
            tool_finished("one"),
            Event::UserInterjected {
                step: 1,
                text: "skip the tests".to_string(),
            },
            says(2, "Skipping them."),
            finished(),
        ];

        let conversation = conversation(&events);

        assert!(matches!(conversation[2].blocks[0], Block::ToolResult(_)));
        assert!(conversation[2].text().ends_with("skip the tests"));
    }

    #[test]
    fn the_gate_feedback_is_kept_only_when_the_model_was_asked_again() {
        let failing = Event::VerificationRan {
            step: 1,
            passed: false,
            checks: vec![crate::verify::CheckOutcome {
                command: "npm test".to_string(),
                passed: false,
                duration_ms: 1,
                output: "1 failed".to_string(),
            }],
            duration_ms: 1,
        };

        let asked_again = [
            started("fix"),
            says(1, "Done."),
            failing.clone(),
            says(2, "Fixed."),
            finished(),
        ];

        let conversation_again = conversation(&asked_again);

        assert_eq!(
            roles(&conversation_again),
            [Role::User, Role::Assistant, Role::User, Role::Assistant]
        );

        assert!(conversation_again[2].text().contains("`npm test` failed"));

        let gave_up = [started("fix"), says(1, "Done."), failing, finished()];

        assert_eq!(
            roles(&conversation(&gave_up)),
            [Role::User, Role::Assistant]
        );
    }
}
