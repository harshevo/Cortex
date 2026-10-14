//! How full the model's context window is.
//!
//! The best measure of a conversation's size is the provider's own count
//! of the input to the latest call, cached tokens included: that is what
//! the model was actually sent, counted by the model's own tokenizer.
//! [`ContextUse`] reports that number, the size of the model's window when
//! it is known, and an estimate of what the input is made of.
//!
//! The split into parts is an estimate. The provider gives one total, not
//! a count per part, so the parts are measured in characters and scaled
//! to the total. Text, code and JSON do not cost the same per character,
//! so the shares are approximate.

use serde::{Deserialize, Serialize};

use crate::compact;
use crate::context::IMAGE_CHARS;
use crate::message::{Block, Message, Role};
use crate::provider::ToolSpec;

/// The heading the first message of a run puts in front of the task when
/// briefings come before it. See [`crate::briefing::compose`].
const TASK_HEADING: &str = "## Task\n\n";

/// Context windows that are known, by model-name prefix, in tokens.
///
/// Only models whose window Anthropic's own reference for its API gave on
/// 2026-10-06 are listed. Models served through a ChatGPT subscription or
/// an OpenAI-compatible server are not: their windows are not published
/// anywhere this code can rely on, and a local server's depends on how it
/// was started. For those, give the window with `--context-window`.
const WINDOWS: [(&str, u64); 10] = [
    ("claude-fable-5", 1_000_000),
    ("claude-mythos-5", 1_000_000),
    ("claude-opus-5", 1_000_000),
    ("claude-opus-4-8", 1_000_000),
    ("claude-opus-4-7", 1_000_000),
    ("claude-opus-4-6", 1_000_000),
    ("claude-sonnet-5", 1_000_000),
    ("claude-sonnet-4-6", 1_000_000),
    ("claude-haiku-5-5", 1_000_000),
    ("claude-haiku-4-5", 200_000),
];

/// The context window of `model` in tokens, when it is known.
pub fn context_window(model: &str) -> Option<u64> {
    WINDOWS
        .iter()
        .filter(|(prefix, _)| model.starts_with(prefix))
        .max_by_key(|(prefix, _)| prefix.len())
        .map(|(_, tokens)| *tokens)
}

/// The sizes of the parts of one request. The unit is characters when
/// measured and tokens when estimated; see [`ContextUse`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextParts {
    /// The system prompt.
    pub system: u64,

    /// The descriptions of the tools.
    pub tool_specs: u64,

    /// What the engine added for the model: briefings in front of a task
    /// and advice between steps.
    pub briefings: u64,

    /// What the person said, and what the harness said in their place
    /// (failed checks, a review).
    pub user: u64,

    /// What the model said. Its sealed reasoning, which some providers
    /// have sent back, is under `other`.
    pub assistant: u64,

    /// The tool calls the model made.
    pub tool_calls: u64,

    /// What the tools returned.
    pub tool_results: u64,

    /// The summary an earlier part of the conversation was replaced by.
    pub summary: u64,

    /// Blocks only the provider understands, such as sealed reasoning.
    pub other: u64,
}

impl ContextParts {
    pub fn total(&self) -> u64 {
        self.system
            + self.tool_specs
            + self.briefings
            + self.user
            + self.assistant
            + self.tool_calls
            + self.tool_results
            + self.summary
            + self.other
    }

    /// These sizes scaled so that they add up to `total`.
    pub fn scaled_to(&self, total: u64) -> Self {
        let own = self.total();

        if own == 0 {
            return Self::default();
        }

        let scale = |part: u64| (u128::from(part) * u128::from(total) / u128::from(own)) as u64;

        Self {
            system: scale(self.system),
            tool_specs: scale(self.tool_specs),
            briefings: scale(self.briefings),
            user: scale(self.user),
            assistant: scale(self.assistant),
            tool_calls: scale(self.tool_calls),
            tool_results: scale(self.tool_results),
            summary: scale(self.summary),
            other: scale(self.other),
        }
    }
}

/// The characters of each part of a request.
pub fn measure(system: &str, tools: &[ToolSpec], conversation: &[Message]) -> ContextParts {
    let mut parts = measure_conversation(conversation);

    parts.system = system.len() as u64;
    parts.tool_specs = tool_spec_chars(tools);

    parts
}

/// The characters the descriptions of `tools` take.
pub fn tool_spec_chars(tools: &[ToolSpec]) -> u64 {
    tools
        .iter()
        .map(|tool| tool.name.len() + tool.description.len() + tool.input_schema.to_string().len())
        .sum::<usize>() as u64
}

fn measure_conversation(conversation: &[Message]) -> ContextParts {
    let mut parts = ContextParts::default();

    for message in conversation {
        let answers_tools = message
            .blocks
            .iter()
            .any(|block| matches!(block, Block::ToolResult(_)));

        for block in &message.blocks {
            match (message.role, block) {
                (Role::User, Block::Text { text }) if compact::is_summary(text) => {
                    parts.summary += text.len() as u64;
                }

                // Text beside tool results is the engine's: advice, or
                // something the person said while the run was going.
                (Role::User, Block::Text { text }) if answers_tools => {
                    parts.briefings += text.len() as u64;
                }

                (Role::User, Block::Text { text }) => {
                    let briefed = text.rfind(TASK_HEADING).unwrap_or(0) as u64;

                    parts.briefings += briefed;
                    parts.user += text.len() as u64 - briefed;
                }

                (Role::Assistant, Block::Text { text }) => parts.assistant += text.len() as u64,

                (_, Block::ToolCall(call)) => {
                    parts.tool_calls += (call.name.len() + call.input.to_string().len()) as u64;
                }

                (_, Block::ToolResult(result)) => {
                    parts.tool_results +=
                        (result.output.len() + result.images.len() * IMAGE_CHARS) as u64;
                }

                (Role::User, Block::Image(_)) => parts.user += IMAGE_CHARS as u64,
                (Role::Assistant, Block::Image(_)) => parts.assistant += IMAGE_CHARS as u64,

                (_, Block::Opaque { raw }) => parts.other += raw.to_string().len() as u64,
            }
        }
    }

    parts
}

/// How full the context window was at the latest model call.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ContextUse {
    /// Tokens of the conversation as last sent, system prompt and tool
    /// descriptions included: the input the provider reported for the
    /// latest call, cached tokens counted. The model's reply to that call
    /// is not in it; `reply_tokens` is.
    pub tokens: u64,

    /// Tokens of the model's reply to that call, which the next call
    /// sends back as well.
    pub reply_tokens: u64,

    /// The model's context window, when it is known or was given.
    pub window: Option<u64>,

    /// `tokens` as a share of the window, from 0 to 1 and beyond.
    pub share: Option<f32>,

    /// What the request was made of, in characters. Measured.
    pub chars: ContextParts,

    /// The same parts in tokens. An estimate: the characters scaled to
    /// `tokens`.
    pub estimated_tokens: ContextParts,

    /// The conversation was shortened after the call these numbers come
    /// from, and `tokens` was brought down by an estimate of what went.
    /// The next call gives a measured figure again.
    #[serde(default)]
    pub estimated: bool,
}

impl ContextUse {
    pub fn new(tokens: u64, reply_tokens: u64, window: Option<u64>, chars: ContextParts) -> Self {
        Self {
            tokens,
            reply_tokens,
            window,
            share: share(tokens, window),
            chars,
            estimated_tokens: chars.scaled_to(tokens),
            estimated: false,
        }
    }

    /// These numbers after `chars_removed` characters were taken out of
    /// the conversation. Returns the tokens that is estimated to have
    /// saved.
    pub fn shortened_by(&mut self, chars_removed: u64) -> u64 {
        let all = self.chars.total();

        if all == 0 {
            return 0;
        }

        let chars_removed = chars_removed.min(all);

        let saved = (u128::from(self.tokens) * u128::from(chars_removed) / u128::from(all)) as u64;

        self.tokens -= saved;
        self.share = share(self.tokens, self.window);
        self.estimated = true;

        saved
    }
}

fn share(tokens: u64, window: Option<u64>) -> Option<f32> {
    window
        .filter(|window| *window > 0)
        .map(|window| tokens as f32 / window as f32)
}

/// One of the windows a subscription's allowance is counted over.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct QuotaWindow {
    /// Share of the allowance used, from 0 to 100.
    pub used_percent: f64,

    /// How long the window is.
    pub window_minutes: Option<u64>,

    /// When the window starts again, in milliseconds since 1970.
    pub resets_at_ms: Option<u64>,
}

/// How much of a subscription's allowance is used, as the provider last
/// said.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct PlanQuota {
    /// The shorter window, usually a few hours.
    pub primary: Option<QuotaWindow>,

    /// The longer window, usually a week.
    pub secondary: Option<QuotaWindow>,

    /// When the provider said so, in milliseconds since 1970.
    pub observed_ms: u64,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::message::{ToolCall, ToolResult};

    #[test]
    fn a_window_is_known_only_for_listed_models() {
        assert_eq!(context_window("claude-opus-5-5"), Some(1_000_000));
        assert_eq!(context_window("claude-haiku-4-5"), Some(200_000));
        assert_eq!(context_window("gpt-6.1-sol"), None);
        assert_eq!(context_window("qwen2.5-coder:7b"), None);
        assert_eq!(context_window("scripted"), None);
    }

    #[test]
    fn each_block_is_counted_under_what_it_is() {
        let call = ToolCall {
            id: "one".to_string(),
            name: "read_file".to_string(),
            input: json!({ "path": "a.rs" }),
        };

        let result = ToolResult {
            call_id: "one".to_string(),
            output: "x".repeat(400),
            is_error: false,
            images: Vec::new(),
        };

        let mut answer = Message::tool_results(vec![result]);

        answer.blocks.push(Block::Text {
            text: "a".repeat(30),
        });

        let conversation = vec![
            Message::user_text(format!("{}## Task\n\n{}", "m".repeat(100), "t".repeat(20))),
            Message {
                role: Role::Assistant,
                blocks: vec![
                    Block::Text {
                        text: "s".repeat(50),
                    },
                    Block::ToolCall(call),
                    Block::Opaque {
                        raw: json!({ "type": "reasoning" }),
                    },
                ],
            },
            answer,
            Message::user_text("u".repeat(7)),
        ];

        let tools = [ToolSpec {
            name: "read_file".to_string(),
            description: "d".repeat(60),
            input_schema: json!({}),
        }];

        let parts = measure(&"p".repeat(1_000), &tools, &conversation);

        assert_eq!(parts.system, 1_000);
        assert_eq!(parts.tool_specs, 9 + 60 + 2);
        assert_eq!(parts.briefings, 100 + 30);

        // The heading and the task itself, then the later message.
        assert_eq!(parts.user, 9 + 20 + 7);
        assert_eq!(parts.assistant, 50);
        assert_eq!(parts.tool_calls, 9 + 15);
        assert_eq!(parts.tool_results, 400);
        assert_eq!(parts.other, 20);
        assert_eq!(parts.summary, 0);
    }

    #[test]
    fn the_estimate_adds_up_to_the_reported_total() {
        let chars = ContextParts {
            system: 3_000,
            tool_results: 6_000,
            user: 1_000,
            ..ContextParts::default()
        };

        let context = ContextUse::new(2_500, 40, Some(10_000), chars);

        assert_eq!(context.estimated_tokens.system, 750);
        assert_eq!(context.estimated_tokens.tool_results, 1_500);
        assert_eq!(context.estimated_tokens.user, 250);
        assert_eq!(context.share, Some(0.25));
        assert!(!context.estimated);

        let unknown = ContextUse::new(2_500, 40, None, chars);

        assert_eq!(unknown.share, None);
    }

    #[test]
    fn shortening_brings_the_figure_down_and_marks_it_estimated() {
        let chars = ContextParts {
            tool_results: 8_000,
            user: 2_000,
            ..ContextParts::default()
        };

        let mut context = ContextUse::new(5_000, 0, Some(10_000), chars);

        assert_eq!(context.shortened_by(4_000), 2_000);
        assert_eq!(context.tokens, 3_000);
        assert_eq!(context.share, Some(0.3));
        assert!(context.estimated);
    }
}
