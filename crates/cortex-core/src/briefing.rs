//! Context gathered before the model sees a task.
//!
//! Memory of earlier work, code found by search, and call sites from the
//! code graph all reach the model the same way: as text placed in front of
//! the instruction. A [`ContextSource`] produces one such piece.

use async_trait::async_trait;
use serde_json::Value;

use crate::workspace::Workspace;

#[derive(Debug, Clone)]
pub struct Briefing {
    /// Which component produced this, for the trajectory log.
    pub source: String,

    /// The text shown to the model.
    pub text: String,

    /// Structured facts about how the text was chosen (which memories, which
    /// route). Logged, never shown to the model; later training data.
    pub detail: Value,
}

#[async_trait]
pub trait ContextSource: Send + Sync {
    /// Context for `instruction`, or `None` when there is nothing useful.
    ///
    /// A source must not fail the run: when something it depends on is
    /// unavailable it returns `None` and the agent works without it.
    async fn brief(&self, instruction: &str, workspace: &Workspace) -> Option<Briefing>;
}

/// The first message of a run: the briefings, then the task itself.
pub fn compose(briefings: &[Briefing], instruction: &str) -> String {
    if briefings.is_empty() {
        return instruction.to_string();
    }

    let mut text = String::new();

    for briefing in briefings {
        text.push_str(briefing.text.trim_end());
        text.push_str("\n\n");
    }

    text.push_str("## Task\n\n");
    text.push_str(instruction);

    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_instruction_without_briefings_is_unchanged() {
        assert_eq!(compose(&[], "fix it"), "fix it");
    }

    #[test]
    fn briefings_come_before_the_task() {
        let briefing = Briefing {
            source: "memory".to_string(),
            text: "## Memory\n\n- tests run with `npm test`\n".to_string(),
            detail: Value::Null,
        };

        let text = compose(&[briefing], "fix it");

        assert!(text.starts_with("## Memory"));
        assert!(text.ends_with("## Task\n\nfix it"));
    }
}
