//! A provider that replays prepared responses.
//!
//! It lets the agent loop, the tools, and the evaluation harness be tested
//! end to end without a network connection or an API key.

use std::collections::VecDeque;
use std::sync::Mutex;

use async_trait::async_trait;
use cortex_core::{
    Block, Message, ModelProvider, ModelRequest, ModelResponse, ProviderError, Role, StopReason,
    ToolCall, Usage,
};
use serde_json::Value;

pub struct ScriptedProvider {
    responses: Mutex<VecDeque<ModelResponse>>,
}

impl ScriptedProvider {
    pub fn new(responses: Vec<ModelResponse>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
        }
    }
}

#[async_trait]
impl ModelProvider for ScriptedProvider {
    fn name(&self) -> &str {
        "scripted"
    }

    fn model(&self) -> &str {
        "scripted"
    }

    async fn complete(&self, _request: ModelRequest<'_>) -> Result<ModelResponse, ProviderError> {
        let mut responses = self
            .responses
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        responses
            .pop_front()
            .ok_or_else(|| ProviderError::Decode("the script has no more responses".to_string()))
    }
}

/// A turn in which the model asks for tools. Each entry is `(tool, input)`.
pub fn tool_turn(calls: Vec<(&str, Value)>) -> ModelResponse {
    let blocks = calls
        .into_iter()
        .enumerate()
        .map(|(index, (name, input))| {
            Block::ToolCall(ToolCall {
                id: format!("call_{index}"),
                name: name.to_string(),
                input,
            })
        })
        .collect();

    ModelResponse {
        message: Message {
            role: Role::Assistant,
            blocks,
        },
        stop_reason: StopReason::ToolUse,
        usage: fixed_usage(),
    }
}

/// A turn in which the model answers and stops.
pub fn final_turn(text: &str) -> ModelResponse {
    ModelResponse {
        message: Message::assistant_text(text),
        stop_reason: StopReason::EndTurn,
        usage: fixed_usage(),
    }
}

fn fixed_usage() -> Usage {
    Usage {
        input_tokens: 100,
        output_tokens: 20,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
    }
}
