//! Answering a repeated lookup without repeating its output.
//!
//! Models re-read. In the measured baseline one file read in five was of a
//! file the run had already read. When a read-only call is the same as an
//! earlier one and the output is the same too, the model already has that
//! text, so the new result is replaced by a line saying so.
//!
//! This is worked out from the conversation alone. Nothing is remembered
//! between runs, and a result is only ever folded onto text that is still
//! in the conversation in full.

use crate::message::{Block, Message, Role, ToolCall, ToolResult};

/// Shorter outputs are cheaper to repeat than to explain.
const MIN_CHARS: usize = 400;

const NOTE: &str = "Unchanged. You made this same call earlier in this conversation and the \
                    result is identical to the one you already have above, so it is not \
                    repeated here.";

/// Whether `output` is the line left in place of a repeated result.
pub fn is_repeat_note(output: &str) -> bool {
    output == NOTE
}

/// Replaces `result` with a short note when `call` repeats an earlier call
/// and got the same output. Returns the characters saved.
pub fn fold(conversation: &[Message], call: &ToolCall, result: &mut ToolResult) -> usize {
    if result.is_error || result.output.len() < MIN_CHARS {
        return 0;
    }

    if !seen_before(conversation, call, &result.output) {
        return 0;
    }

    let saved = result.output.len().saturating_sub(NOTE.len());

    result.output = NOTE.to_string();

    saved
}

fn seen_before(conversation: &[Message], call: &ToolCall, output: &str) -> bool {
    answered_calls(conversation).any(|(earlier, result)| {
        earlier.name == call.name && earlier.input == call.input && result.output == output
    })
}

/// Every tool call in the conversation together with its result.
///
/// A call is answered in the message right after the one that made it.
/// Pairing them that way, instead of by id across the whole conversation,
/// stays correct for providers that reuse ids from one response to the next.
pub fn answered_calls(
    conversation: &[Message],
) -> impl Iterator<Item = (&ToolCall, &ToolResult)> + '_ {
    conversation.windows(2).flat_map(|pair| {
        let (asking, answering) = (&pair[0], &pair[1]);

        let calls = asking
            .blocks
            .iter()
            .filter(move |_| asking.role == Role::Assistant)
            .filter_map(|block| match block {
                Block::ToolCall(call) => Some(call),
                _ => None,
            });

        calls.filter_map(move |call| Some((call, result_for(answering, &call.id)?)))
    })
}

fn result_for<'a>(message: &'a Message, call_id: &str) -> Option<&'a ToolResult> {
    message.blocks.iter().find_map(|block| match block {
        Block::ToolResult(result) if result.call_id == call_id => Some(result),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn read(path: &str) -> ToolCall {
        ToolCall {
            id: "call_0".to_string(),
            name: "read_file".to_string(),
            input: json!({ "path": path }),
        }
    }

    fn result(output: &str) -> ToolResult {
        ToolResult {
            call_id: "call_0".to_string(),
            output: output.to_string(),
            is_error: false,
            images: Vec::new(),
        }
    }

    fn turn(call: ToolCall, output: &str) -> [Message; 2] {
        [
            Message {
                role: Role::Assistant,
                blocks: vec![Block::ToolCall(call)],
            },
            Message::tool_results(vec![result(output)]),
        ]
    }

    #[test]
    fn the_same_read_with_the_same_text_is_folded() {
        let text = "x".repeat(2_000);

        let conversation = turn(read("a.py"), &text);

        let mut again = result(&text);

        let saved = fold(&conversation, &read("a.py"), &mut again);

        assert!(saved > 1_500);
        assert!(is_repeat_note(&again.output));
    }

    #[test]
    fn a_file_that_changed_is_shown_again() {
        let conversation = turn(read("a.py"), &"x".repeat(2_000));

        let changed = "y".repeat(2_000);

        let mut again = result(&changed);

        assert_eq!(fold(&conversation, &read("a.py"), &mut again), 0);
        assert_eq!(again.output, changed);
    }

    #[test]
    fn another_file_with_the_same_text_is_shown() {
        let text = "x".repeat(2_000);

        let conversation = turn(read("a.py"), &text);

        let mut other = result(&text);

        assert_eq!(fold(&conversation, &read("b.py"), &mut other), 0);
    }

    #[test]
    fn short_results_and_errors_are_left_alone() {
        let conversation = turn(read("a.py"), "short");

        let mut short = result("short");

        assert_eq!(fold(&conversation, &read("a.py"), &mut short), 0);

        let text = "x".repeat(2_000);

        let conversation = turn(read("a.py"), &text);

        let mut failed = ToolResult {
            is_error: true,
            images: Vec::new(),
            ..result(&text)
        };

        assert_eq!(fold(&conversation, &read("a.py"), &mut failed), 0);
    }
}
