//! Keeping the conversation from carrying text that no longer matters.
//!
//! Every step re-sends the whole conversation, so a large tool result is
//! paid for again on every later step. Some of those results go stale while
//! a run is still going: a file is read, edited, and read again; a test
//! command is run, the code is fixed, and the command is run again. The
//! older result then describes something that no longer exists.
//!
//! Pruning replaces such superseded results with one line. Unlike trimming
//! by age, it never removes the only copy of something the model may still
//! be working from, so it does not send the model back to read a file again.
//!
//! Masking is the blunter tool for long runs: tool output older than the
//! last few steps is replaced with one line, whatever it was, while the
//! model's own reasoning and calls stay. Measured on SWE-bench Verified
//! ("The Complexity Trap", Lindenbauer et al., 2025) this halves cost at
//! the same solve rate as summarising with a model, and it needs no model
//! call. It can remove the only copy of something; the model is told it
//! can make the call again.
//!
//! Two things to know before turning either on:
//!
//! - It rewrites earlier messages, which invalidates a provider's prompt
//!   cache from the first rewritten message on. To limit that, nothing is
//!   touched until enough can be reclaimed to be worth one cache miss, and
//!   then everything superseded is removed at once.
//! - Some providers reject a conversation whose history was edited (the
//!   Anthropic API does for its newest models). Do not use it with them.

use crate::message::{Block, Message, Role, ToolCall};
use crate::repeat;

#[derive(Debug, Clone)]
pub struct PruneConfig {
    /// Tool results shorter than this are never removed.
    pub min_chars: usize,

    /// Nothing is removed until at least this many characters can be
    /// reclaimed in one pass.
    pub min_reclaim_chars: usize,
}

impl Default for PruneConfig {
    fn default() -> Self {
        Self {
            min_chars: 1_200,

            // Roughly 1,500 tokens, re-sent on every remaining step.
            min_reclaim_chars: 6_000,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PruneStats {
    pub results_trimmed: usize,
    pub chars_removed: usize,
}

const NOTE: &str = "[This older result was removed because the same call was made again \
                    later. The newer result further down is the current one.]";

#[derive(Debug, Clone)]
pub struct MaskConfig {
    /// Output from this many of the latest steps that called tools is
    /// always kept.
    pub keep_steps: usize,

    /// Tool results shorter than this are never masked.
    pub min_chars: usize,

    /// Nothing is masked until this many results are due, so the
    /// provider's prompt cache is broken once per batch, not every step.
    pub batch: usize,
}

impl Default for MaskConfig {
    fn default() -> Self {
        Self {
            // The window the paper found best for SWE-agent was 10 turns.
            keep_steps: 10,
            min_chars: 600,
            batch: 4,
        }
    }
}

const MASK_NOTE: &str = "[The output of this earlier call was removed to keep the conversation \
                         short. Make the call again if you need it.]";

/// Whether `output` is the line left in place of masked output.
pub fn is_mask_note(output: &str) -> bool {
    output == MASK_NOTE
}

/// Replaces tool output older than the last few steps with one line.
pub fn mask(conversation: &mut [Message], config: &MaskConfig) -> PruneStats {
    let due = maskable_results(conversation, config);

    if due.len() < config.batch.max(1) {
        return PruneStats::default();
    }

    let mut stats = PruneStats::default();

    for place in due {
        let Block::ToolResult(result) = &mut conversation[place.message].blocks[place.block] else {
            continue;
        };

        result.output = MASK_NOTE.to_string();
        result.images.clear();

        stats.results_trimmed += 1;
        stats.chars_removed += place.chars.saturating_sub(MASK_NOTE.len());
    }

    stats
}

/// The results old and long enough to mask.
fn maskable_results(conversation: &[Message], config: &MaskConfig) -> Vec<Place> {
    // Messages that called tools, oldest first. The results of the last
    // `keep_steps` of them are inside the window.
    let calling: Vec<usize> = conversation
        .iter()
        .enumerate()
        .filter(|(_, message)| message.role == Role::Assistant && !message.tool_calls().is_empty())
        .map(|(index, _)| index)
        .collect();

    let Some(outside) = calling.len().checked_sub(config.keep_steps) else {
        return Vec::new();
    };

    let Some(&window_start) = calling.get(outside) else {
        return Vec::new();
    };

    let answered = answered_calls(conversation);

    let mut due = Vec::new();

    for (position, earlier) in answered.iter().enumerate() {
        let output = result_at(conversation, earlier.place);

        let is_note = output == NOTE || output == MASK_NOTE || repeat::is_repeat_note(output);

        if earlier.place.message > window_start || is_note || earlier.place.chars < config.min_chars
        {
            continue;
        }

        // A later repeat of this call was answered with "unchanged, see
        // above". This result is the "above" and has to stay.
        let pointed_to = answered[position + 1..].iter().any(|later| {
            later.call.name == earlier.call.name
                && later.call.input == earlier.call.input
                && repeat::is_repeat_note(result_at(conversation, later.place))
        });

        if !pointed_to {
            due.push(earlier.place);
        }
    }

    due
}

fn result_at(conversation: &[Message], place: Place) -> &str {
    match &conversation[place.message].blocks[place.block] {
        Block::ToolResult(result) => &result.output,
        _ => "",
    }
}

/// Removes superseded tool results once enough of them have built up.
pub fn prune(conversation: &mut [Message], config: &PruneConfig) -> PruneStats {
    let superseded = superseded_results(conversation, config.min_chars);

    let reclaimable: usize = superseded.iter().map(|place| place.chars).sum();

    if reclaimable < config.min_reclaim_chars {
        return PruneStats::default();
    }

    let mut stats = PruneStats::default();

    for place in superseded {
        let Block::ToolResult(result) = &mut conversation[place.message].blocks[place.block] else {
            continue;
        };

        result.output = NOTE.to_string();
        result.images.clear();

        stats.results_trimmed += 1;
        stats.chars_removed += place.chars.saturating_sub(NOTE.len());
    }

    stats
}

/// A picture is billed by its size on screen, not its bytes; this is about
/// what a full-size one costs.
const IMAGE_CHARS: usize = 6_000;

/// Where one tool result sits in the conversation.
#[derive(Clone, Copy)]
struct Place {
    message: usize,
    block: usize,
    chars: usize,
}

/// One tool call and where its result is.
struct Answered<'a> {
    call: &'a ToolCall,
    place: Place,

    /// Whether the result holds real output that can stand in for an
    /// earlier one: not an error, and not a note left by this module or by
    /// [`repeat`].
    current: bool,
}

/// The results that a later, identical call has replaced.
fn superseded_results(conversation: &[Message], min_chars: usize) -> Vec<Place> {
    let answered = answered_calls(conversation);

    let mut superseded = Vec::new();

    for (position, earlier) in answered.iter().enumerate() {
        if !earlier.current || earlier.place.chars < min_chars {
            continue;
        }

        let replaced = answered[position + 1..].iter().any(|later| {
            later.current
                && later.call.name == earlier.call.name
                && later.call.input == earlier.call.input
        });

        if replaced {
            superseded.push(earlier.place);
        }
    }

    superseded
}

/// Every answered call, in the order the calls were made. A call is
/// answered in the message right after the one that made it.
fn answered_calls(conversation: &[Message]) -> Vec<Answered<'_>> {
    let mut answered = Vec::new();

    for (index, asking) in conversation.iter().enumerate() {
        let Some(answering) = conversation.get(index + 1) else {
            break;
        };

        if asking.role != Role::Assistant {
            continue;
        }

        for block in &asking.blocks {
            let Block::ToolCall(call) = block else {
                continue;
            };

            let found = answering
                .blocks
                .iter()
                .enumerate()
                .find_map(|(at, block)| match block {
                    Block::ToolResult(result) if result.call_id == call.id => Some((at, result)),
                    _ => None,
                });

            let Some((block, result)) = found else {
                continue;
            };

            let is_note = result.output == NOTE
                || result.output == MASK_NOTE
                || repeat::is_repeat_note(&result.output);

            answered.push(Answered {
                call,
                place: Place {
                    message: index + 1,
                    block,
                    chars: result.output.len() + result.images.len() * IMAGE_CHARS,
                },
                current: !result.is_error && !is_note,
            });
        }
    }

    answered
}

/// A rough size for the conversation. Characters are a good enough stand-in
/// for tokens when comparing two runs.
pub fn conversation_chars(conversation: &[Message]) -> usize {
    conversation
        .iter()
        .flat_map(|message| &message.blocks)
        .map(|block| match block {
            Block::Text { text } => text.len(),
            Block::ToolCall(call) => call.name.len() + call.input.to_string().len(),
            Block::ToolResult(result) => result.output.len() + result.images.len() * IMAGE_CHARS,
            Block::Image(_) => IMAGE_CHARS,
            Block::Opaque { raw } => raw.to_string().len(),
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::message::ToolResult;

    /// A call and its answer, as the two messages the loop would add.
    fn turn(tool: &str, input: Value, output: &str) -> [Message; 2] {
        let call = ToolCall {
            id: "call_0".to_string(),
            name: tool.to_string(),
            input,
        };

        let result = ToolResult {
            call_id: "call_0".to_string(),
            output: output.to_string(),
            is_error: false,
            images: Vec::new(),
        };

        [
            Message {
                role: Role::Assistant,
                blocks: vec![Block::ToolCall(call)],
            },
            Message::tool_results(vec![result]),
        ]
    }

    fn output(conversation: &[Message], message: usize) -> &str {
        match &conversation[message].blocks[0] {
            Block::ToolResult(result) => &result.output,
            _ => panic!("expected a tool result"),
        }
    }

    fn config() -> PruneConfig {
        PruneConfig {
            min_chars: 1_000,
            min_reclaim_chars: 3_000,
        }
    }

    #[test]
    fn a_read_made_again_after_an_edit_replaces_the_first() {
        let before = "a".repeat(4_000);
        let after = "b".repeat(4_000);

        let mut conversation = Vec::new();

        conversation.extend(turn("read_file", json!({ "path": "a.py" }), &before));
        conversation.extend(turn("edit_file", json!({ "path": "a.py" }), "edited a.py"));
        conversation.extend(turn("read_file", json!({ "path": "a.py" }), &after));

        let stats = prune(&mut conversation, &config());

        assert_eq!(stats.results_trimmed, 1);
        assert_eq!(output(&conversation, 1), NOTE);
        assert_eq!(output(&conversation, 5), after);

        // A second pass finds nothing left to do.
        assert_eq!(prune(&mut conversation, &config()), PruneStats::default());
    }

    #[test]
    fn a_result_nothing_replaced_is_kept_however_old() {
        let mut conversation = Vec::new();

        conversation.extend(turn(
            "read_file",
            json!({ "path": "a.py" }),
            &"a".repeat(9_000),
        ));

        for step in 0..10 {
            let path = format!("other-{step}.py");

            conversation.extend(turn(
                "read_file",
                json!({ "path": path }),
                &"b".repeat(2_000),
            ));
        }

        assert_eq!(prune(&mut conversation, &config()), PruneStats::default());
        assert_eq!(output(&conversation, 1).len(), 9_000);
    }

    #[test]
    fn nothing_is_touched_until_enough_can_be_reclaimed() {
        let mut conversation = Vec::new();

        conversation.extend(turn(
            "shell",
            json!({ "command": "npm test" }),
            &"a".repeat(1_500),
        ));
        conversation.extend(turn(
            "shell",
            json!({ "command": "npm test" }),
            &"b".repeat(1_500),
        ));

        assert_eq!(prune(&mut conversation, &config()), PruneStats::default());

        conversation.extend(turn(
            "shell",
            json!({ "command": "npm test" }),
            &"c".repeat(1_500),
        ));

        let stats = prune(&mut conversation, &config());

        assert_eq!(stats.results_trimmed, 2);
        assert_eq!(output(&conversation, 5).len(), 1_500);
    }

    #[test]
    fn a_repeat_note_does_not_replace_the_text_it_points_to() {
        let text = "a".repeat(4_000);

        let mut conversation = Vec::new();

        conversation.extend(turn("read_file", json!({ "path": "a.py" }), &text));

        let call = ToolCall {
            id: "call_0".to_string(),
            name: "read_file".to_string(),
            input: json!({ "path": "a.py" }),
        };

        let mut again = ToolResult {
            call_id: "call_0".to_string(),
            output: text.clone(),
            is_error: false,
            images: Vec::new(),
        };

        repeat::fold(&conversation, &call, &mut again);

        conversation.push(Message {
            role: Role::Assistant,
            blocks: vec![Block::ToolCall(call)],
        });
        conversation.push(Message::tool_results(vec![again]));

        assert_eq!(prune(&mut conversation, &config()), PruneStats::default());
        assert_eq!(output(&conversation, 1), text);
    }

    fn mask_config() -> MaskConfig {
        MaskConfig {
            keep_steps: 2,
            min_chars: 1_000,
            batch: 2,
        }
    }

    #[test]
    fn output_older_than_the_window_is_masked_in_batches() {
        let mut conversation = vec![Message::user_text("the task")];

        for step in 0..3 {
            let path = format!("file-{step}.py");

            conversation.extend(turn(
                "read_file",
                json!({ "path": path }),
                &"a".repeat(2_000),
            ));
        }

        // One result is outside the window: not yet a batch.
        assert_eq!(
            mask(&mut conversation, &mask_config()),
            PruneStats::default()
        );

        conversation.extend(turn(
            "read_file",
            json!({ "path": "last.py" }),
            &"b".repeat(2_000),
        ));

        let stats = mask(&mut conversation, &mask_config());

        assert_eq!(stats.results_trimmed, 2);
        assert!(is_mask_note(output(&conversation, 2)));
        assert!(is_mask_note(output(&conversation, 4)));
        assert_eq!(output(&conversation, 6).len(), 2_000);
        assert_eq!(output(&conversation, 8).len(), 2_000);

        // The task and the model's calls are untouched.
        assert_eq!(conversation[0].text(), "the task");
        assert_eq!(
            conversation[1].tool_calls()[0].input,
            json!({ "path": "file-0.py" })
        );
    }

    #[test]
    fn short_output_and_text_a_repeat_note_points_to_are_not_masked() {
        let text = "a".repeat(4_000);

        let mut conversation = Vec::new();

        conversation.extend(turn("read_file", json!({ "path": "a.py" }), &text));
        conversation.extend(turn("shell", json!({ "command": "ls" }), "a.py"));
        conversation.extend(turn(
            "read_file",
            json!({ "path": "b.py" }),
            &"b".repeat(4_000),
        ));

        let call = ToolCall {
            id: "call_0".to_string(),
            name: "read_file".to_string(),
            input: json!({ "path": "a.py" }),
        };

        let mut again = ToolResult {
            call_id: "call_0".to_string(),
            output: text.clone(),
            is_error: false,
            images: Vec::new(),
        };

        repeat::fold(&conversation, &call, &mut again);

        conversation.push(Message {
            role: Role::Assistant,
            blocks: vec![Block::ToolCall(call)],
        });
        conversation.push(Message::tool_results(vec![again]));

        conversation.extend(turn("shell", json!({ "command": "pwd" }), "/tmp"));
        conversation.extend(turn("shell", json!({ "command": "id" }), "me"));

        let config = MaskConfig {
            batch: 1,
            ..mask_config()
        };

        let stats = mask(&mut conversation, &config);

        // Only b.py: a.py is what the repeat note refers to, `ls` is short.
        assert_eq!(stats.results_trimmed, 1);
        assert_eq!(output(&conversation, 1), text);
        assert_eq!(output(&conversation, 3), "a.py");
        assert!(is_mask_note(output(&conversation, 5)));
    }
}
