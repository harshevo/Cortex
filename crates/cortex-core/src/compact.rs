//! Replacing the older part of a conversation with a summary.
//!
//! Pruning and masking ([`crate::context`]) take tool output out of the
//! conversation and leave everything else. That is not enough for a
//! session that goes on for hours: the model's own messages and its calls
//! add up as well. Compacting asks the model, in one extra call, to write
//! down what the older part of the conversation established, and puts
//! that summary in its place. The most recent responses, with the output
//! of their tools, stay as they are.
//!
//! The same two cautions apply as to pruning and masking, and a third:
//!
//! - It rewrites the start of the conversation, so the provider's prompt
//!   cache is of no use for the next call beyond the system prompt and
//!   the tools. One compaction costs one summary call and one uncached
//!   call; it pays for itself over the steps that follow.
//! - The Anthropic API ties a model's reasoning to the exact conversation
//!   that produced it and rejects it after the history before it has
//!   changed. Kept responses would carry such reasoning, so with that
//!   provider nothing is kept: the summary replaces the whole
//!   conversation, which is the shape that API accepts. See
//!   [`CompactConfig::for_provider`].
//! - A summary loses detail. The model is told that what it read before
//!   is gone and that it can read it again.
//!
//! A tool result is never left without its call: the part that is kept
//! always begins with a response of the model, so every result in it
//! answers a call that is also in it.

use std::time::Instant;

use crate::agent::Agent;
use crate::context::conversation_chars;
use crate::error::ProviderError;
use crate::judge;
use crate::message::{Block, Message, Role};
use crate::provider::ModelProvider;
use crate::trajectory::Event;
use crate::usage::Usage;
use crate::window;

/// How the summary message begins. It is how a summary is told from what
/// the person said.
const HEADING: &str = "## Summary of the conversation so far";

const NOTE: &str = "[The earlier part of this conversation was replaced by this summary to keep \
                    it short. Files and command output it mentions are no longer in front of \
                    you: read a file or run a command again when you need what it said.]";

const CARRY_ON: &str = "Carry on with the task from where the summary leaves off.";

/// The summariser is shown at most this much of one tool result, of one
/// call's input, and of one piece of text.
const MAX_RESULT_CHARS: usize = 1_500;
const MAX_INPUT_CHARS: usize = 600;
const MAX_TEXT_CHARS: usize = 6_000;

/// The summariser is shown at most this much in all when the model's
/// window is known to be large, and this much when it is not known.
const MAX_TRANSCRIPT_CHARS: usize = 200_000;
const MAX_TRANSCRIPT_CHARS_UNKNOWN_WINDOW: usize = 48_000;

const SYSTEM: &str = "\
You are given the record of the earlier part of a conversation between a person and a coding \
agent that works in the person's project with tools. That part is about to be removed from \
the conversation and replaced by what you write, so the agent that carries on will know only \
what you put down. Write for that agent. Be exact and brief; leave out pleasantries and \
anything that no longer matters.

Use these headings, in this order, and write \"None.\" under one that has nothing:

## Requests
What the person asked for, in order. Quote a request word for word when it is short; \
otherwise keep every requirement and constraint in it. Mark each as done, in progress, or \
not started.

## Decisions
Choices that were made and why, including approaches that were tried and abandoned, and \
anything the person said to do or not to do.

## Files
Each file that was created, changed or deleted, with what was changed in it. Then the files \
that were only read and matter for what comes next, with what was learned from them.

## Commands
The commands that were run and how each ended the last time it was run: passed, failed and \
with what error. Leave out earlier runs that a later run replaced.

## Open problems
Failures not yet fixed, questions not yet answered, and things that could not be verified.

## Next
What the agent was about to do when this part of the conversation ended.

If the record begins with an earlier summary, carry over everything in it that still holds.";

/// When and how much to compact.
#[derive(Debug, Clone, PartialEq)]
pub struct CompactConfig {
    /// The most recent responses of the model that are kept as they are,
    /// together with the output of their tools. With 0 the summary
    /// replaces the whole conversation.
    pub keep_recent: usize,

    /// Compact on its own account when the input of the last model call
    /// was at least this share of the context window. `None` leaves it
    /// to be asked for. It never happens for a model whose window is not
    /// known.
    pub auto_at: Option<f32>,
}

/// The share of the window at which a conversation is compacted unless
/// another is given.
pub const DEFAULT_AUTO_AT: f32 = 0.8;

/// The responses kept unless another number is given.
pub const DEFAULT_KEEP_RECENT: usize = 6;

impl Default for CompactConfig {
    fn default() -> Self {
        Self {
            keep_recent: DEFAULT_KEEP_RECENT,
            auto_at: Some(DEFAULT_AUTO_AT),
        }
    }
}

impl CompactConfig {
    /// The settings that suit the provider of this name. For the
    /// Anthropic API nothing is kept beside the summary, because that
    /// API rejects a model's reasoning once the conversation before it
    /// has changed, and the kept responses would carry it.
    pub fn for_provider(provider: &str) -> Self {
        Self::default().suited_to(provider)
    }

    /// These settings, changed where the provider of this name needs it.
    pub fn suited_to(mut self, provider: &str) -> Self {
        if provider == "anthropic" {
            self.keep_recent = 0;
        }

        self
    }

    /// Only when asked for, never on its own account.
    pub fn manual(mut self) -> Self {
        self.auto_at = None;

        self
    }
}

/// What a compaction did.
#[derive(Debug, Clone, PartialEq)]
pub struct Compaction {
    /// The message now at the start of the conversation.
    pub text: String,

    /// Messages the summary took the place of.
    pub messages_replaced: usize,

    /// Messages at the end of the conversation that were kept.
    pub kept: usize,

    /// Characters of the conversation before and after.
    pub chars_before: usize,
    pub chars_after: usize,

    /// Tokens the summary call used.
    pub usage: Usage,

    pub duration_ms: u64,
}

impl Compaction {
    pub fn chars_removed(&self) -> usize {
        self.chars_before.saturating_sub(self.chars_after)
    }
}

/// Whether `text` is the summary an earlier part of a conversation was
/// replaced by.
pub fn is_summary(text: &str) -> bool {
    text.starts_with(HEADING)
}

/// How many messages at the start of `conversation` a compaction that
/// keeps `keep_recent` responses would replace. `None` when there is too
/// little before them to be worth a summary.
pub fn replaceable(conversation: &[Message], keep_recent: usize) -> Option<usize> {
    let responses: Vec<usize> = conversation
        .iter()
        .enumerate()
        .filter(|(_, message)| message.role == Role::Assistant)
        .map(|(index, _)| index)
        .collect();

    // The kept part begins with a response, so that every tool result in
    // it follows its call.
    let cut = match keep_recent {
        0 => conversation.len(),
        keep => *responses.get(responses.len().checked_sub(keep)?)?,
    };

    // One message is not shorter as a summary, and a summary of a
    // summary alone says nothing new.
    let head = &conversation[..cut];

    let only_a_summary = head.len() == 1 && is_summary(&head[0].text());

    (head.len() >= 2 || (head.len() == 1 && keep_recent == 0 && !only_a_summary)).then_some(cut)
}

/// Puts a summary in place of everything but the last `kept` messages.
/// `text` is the whole message, as [`compact`] returned it and as the
/// log recorded it.
pub fn apply(conversation: &mut Vec<Message>, kept: usize, text: &str) {
    let cut = conversation.len().saturating_sub(kept);

    conversation.splice(..cut, [Message::user_text(text)]);
}

/// Compacts `conversation` with one call to `provider`.
///
/// Returns `None`, having called nothing, when there is too little to
/// summarise. When the call fails, or the model writes nothing, the
/// conversation is left exactly as it was.
///
/// `mid_run` says the model is in the middle of a task and will be asked
/// again straight away, without a new instruction from the person.
pub async fn compact(
    provider: &dyn ModelProvider,
    conversation: &mut Vec<Message>,
    config: &CompactConfig,
    context_window: Option<u64>,
    mid_run: bool,
) -> Result<Option<Compaction>, ProviderError> {
    let Some(cut) = replaceable(conversation, config.keep_recent) else {
        return Ok(None);
    };

    let started = Instant::now();

    let max_chars = match context_window {
        // Half the window, at some three characters to a token.
        Some(window) => (window as usize / 2 * 3).min(MAX_TRANSCRIPT_CHARS),
        None => MAX_TRANSCRIPT_CHARS_UNKNOWN_WINDOW,
    };

    let record = judge::clip_middle(&transcript(&conversation[..cut]), max_chars);

    let answer = judge::ask(provider, SYSTEM, &record).await?;

    let summary = answer.text.trim();

    if summary.is_empty() {
        return Err(ProviderError::Decode(
            "the model wrote no summary".to_string(),
        ));
    }

    let kept = conversation.len() - cut;

    let mut text = format!("{HEADING}\n\n{NOTE}\n\n{summary}");

    // With nothing kept and no new instruction coming, the summary is
    // all the model has to go on.
    if mid_run && kept == 0 {
        text.push_str("\n\n");
        text.push_str(CARRY_ON);
    }

    let chars_before = conversation_chars(conversation);

    apply(conversation, kept, &text);

    Ok(Some(Compaction {
        text,
        messages_replaced: cut,
        kept,
        chars_before,
        chars_after: conversation_chars(conversation),
        usage: answer.usage,
        duration_ms: started.elapsed().as_millis() as u64,
    }))
}

/// The part of a conversation that is to be summarised, as text for the
/// model that writes the summary.
fn transcript(messages: &[Message]) -> String {
    let mut text = String::new();

    for message in messages {
        for block in &message.blocks {
            match (message.role, block) {
                (_, Block::Text { text: said }) if said.trim().is_empty() => {}

                (Role::User, Block::Text { text: said }) if is_summary(said) => {
                    text.push_str(&format!("EARLIER SUMMARY:\n{said}\n\n"));
                }

                (Role::User, Block::Text { text: said }) => {
                    let said = judge::clip_middle(said, MAX_TEXT_CHARS);

                    text.push_str(&format!("PERSON:\n{said}\n\n"));
                }

                (Role::Assistant, Block::Text { text: said }) => {
                    let said = judge::clip_middle(said, MAX_TEXT_CHARS);

                    text.push_str(&format!("AGENT:\n{said}\n\n"));
                }

                (_, Block::ToolCall(call)) => {
                    let input = judge::clip_middle(&call.input.to_string(), MAX_INPUT_CHARS);

                    text.push_str(&format!("AGENT CALLED {} {input}\n\n", call.name));
                }

                (_, Block::ToolResult(result)) => {
                    let output = judge::clip_middle(&result.output, MAX_RESULT_CHARS);
                    let failed = if result.is_error { " (FAILED)" } else { "" };

                    text.push_str(&format!("RESULT{failed}:\n{output}\n\n"));
                }

                (_, Block::Image(_)) => text.push_str("[a picture]\n\n"),

                // Sealed reasoning says nothing to another call.
                (_, Block::Opaque { .. }) => {}
            }
        }
    }

    text
}

impl Agent {
    /// The settings a compaction of this agent's conversation uses: its
    /// own when the capability is on, and otherwise the ones that suit
    /// its provider.
    pub fn compact_config(&self) -> CompactConfig {
        self.config
            .extras
            .compact
            .clone()
            .unwrap_or_else(|| CompactConfig::for_provider(self.provider.name()))
    }

    /// The model's context window: the one given, or else the one known
    /// for the model.
    pub fn context_window(&self) -> Option<u64> {
        self.config
            .extras
            .context_window
            .or_else(|| window::context_window(self.provider.model()))
    }

    /// Compacts the conversation of a session that is idle, because the
    /// person asked for it. Works whether or not the capability is on.
    ///
    /// Do not call it while a run is using the conversation. After a run
    /// that was abandoned part-way, call
    /// [`crate::agent::close_unanswered_calls`] first.
    ///
    /// Returns `None` when there was too little to summarise. On an
    /// error the conversation is as it was. What was done is told to the
    /// agent's sinks, so the session's log replays to the same
    /// conversation.
    pub async fn compact(
        &self,
        conversation: &mut Vec<Message>,
    ) -> Result<Option<Compaction>, ProviderError> {
        self.compact_at(0, conversation, false).await
    }

    /// Compacts and records it. `step` is the step about to be asked, or
    /// 0 between runs.
    pub(crate) async fn compact_at(
        &self,
        step: u32,
        conversation: &mut Vec<Message>,
        automatic: bool,
    ) -> Result<Option<Compaction>, ProviderError> {
        let done = compact(
            self.helper(),
            conversation,
            &self.compact_config(),
            self.context_window(),
            step > 0,
        )
        .await?;

        if let Some(compaction) = &done {
            self.emit(Event::ContextCompacted {
                step,
                kept: compaction.kept,
                text: compaction.text.clone(),
                messages_replaced: compaction.messages_replaced,
                chars_before: compaction.chars_before,
                chars_after: compaction.chars_after,
                usage: compaction.usage,
                duration_ms: compaction.duration_ms,
                automatic,
            });
        }

        Ok(done)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::message::{ToolCall, ToolResult};

    fn step(id: &str, output: &str) -> [Message; 2] {
        [
            Message {
                role: Role::Assistant,
                blocks: vec![Block::ToolCall(ToolCall {
                    id: id.to_string(),
                    name: "read_file".to_string(),
                    input: json!({ "path": format!("{id}.rs") }),
                })],
            },
            Message::tool_results(vec![ToolResult {
                call_id: id.to_string(),
                output: output.to_string(),
                is_error: false,
                images: Vec::new(),
            }]),
        ]
    }

    fn long_conversation() -> Vec<Message> {
        let mut conversation = vec![Message::user_text("Fix the rounding.")];

        for id in ["a", "b", "c", "d"] {
            conversation.extend(step(id, "contents"));
        }

        conversation
    }

    #[test]
    fn the_kept_part_begins_with_a_response() {
        let conversation = long_conversation();

        // Two responses kept: the cut falls on the third response.
        assert_eq!(replaceable(&conversation, 2), Some(5));
        assert_eq!(conversation[5].role, Role::Assistant);

        assert_eq!(replaceable(&conversation, 0), Some(9));
    }

    #[test]
    fn too_little_is_not_compacted() {
        let conversation = long_conversation();

        // Everything but the task would be kept.
        assert_eq!(replaceable(&conversation, 4), None);
        assert_eq!(replaceable(&conversation, 9), None);
        assert_eq!(replaceable(&[], 0), None);

        // A summary alone is not summarised again.
        let mut summarised = conversation.clone();

        apply(&mut summarised, 0, &format!("{HEADING}\n\nAll done."));

        assert_eq!(summarised.len(), 1);
        assert_eq!(replaceable(&summarised, 0), None);
    }

    #[test]
    fn applying_keeps_the_tail_and_puts_the_summary_first() {
        let mut conversation = long_conversation();

        let tail = conversation[5..].to_vec();

        apply(&mut conversation, 4, "the summary");

        assert_eq!(conversation.len(), 5);
        assert_eq!(conversation[0], Message::user_text("the summary"));
        assert_eq!(conversation[1..], tail);

        // A log may record more kept messages than a replayed
        // conversation has; that must not panic.
        apply(&mut conversation, 99, "again");

        assert_eq!(conversation.len(), 6);
    }

    #[test]
    fn the_summariser_sees_who_said_what_and_long_output_shortened() {
        let mut conversation = vec![Message::user_text("Fix the rounding.")];

        conversation.extend(step("a", &"x".repeat(9_000)));
        conversation.push(Message::assistant_text("It rounds down."));

        let record = transcript(&conversation);

        assert!(record.starts_with("PERSON:\nFix the rounding.\n\n"));
        assert!(record.contains("AGENT CALLED read_file {\"path\":\"a.rs\"}"));
        assert!(record.contains("characters omitted"));
        assert!(record.ends_with("AGENT:\nIt rounds down.\n\n"));
        assert!(record.len() < 3_000);
    }

    #[test]
    fn the_anthropic_api_gets_a_summary_and_nothing_kept() {
        assert_eq!(CompactConfig::for_provider("anthropic").keep_recent, 0);
        assert_eq!(
            CompactConfig::for_provider("codex").keep_recent,
            DEFAULT_KEEP_RECENT
        );
        assert_eq!(CompactConfig::default().manual().auto_at, None);
    }
}
