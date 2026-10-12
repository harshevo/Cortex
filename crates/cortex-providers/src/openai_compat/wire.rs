//! Conversion between Cortex types and chat-completions JSON.

use cortex_core::{
    Block, Message, ModelRequest, ModelResponse, ProviderError, Role, StopReason, ToolCall,
    ToolResult, ToolSpec, Usage,
};
use serde_json::{Value, json};

use super::OpenAiCompatConfig;
use super::text_tool_calls;

pub fn build_request_body(config: &OpenAiCompatConfig, request: &ModelRequest<'_>) -> Value {
    let mut messages = vec![json!({ "role": "system", "content": request.system })];

    for message in request.messages {
        messages.extend(message_to_wire(message));
    }

    let tools: Vec<Value> = request.tools.iter().map(tool_to_wire).collect();

    let mut body = json!({
        "model": config.model,
        "messages": messages,
        "max_tokens": config.max_tokens,
        "stream": false,
    });

    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
    }

    if let Some(temperature) = config.temperature {
        body["temperature"] = json!(temperature);
    }

    body
}

fn tool_to_wire(spec: &ToolSpec) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": spec.name,
            "description": spec.description,
            "parameters": spec.input_schema,
        },
    })
}

/// One Cortex message can become several wire messages, because this format
/// carries each tool result as its own `tool` message.
fn message_to_wire(message: &Message) -> Vec<Value> {
    match message.role {
        Role::Assistant => vec![assistant_to_wire(message)],
        Role::User => user_to_wire(message),
    }
}

fn assistant_to_wire(message: &Message) -> Value {
    let calls: Vec<Value> = message.tool_calls().iter().map(call_to_wire).collect();

    let mut wire = json!({
        "role": "assistant",
        "content": message.text(),
    });

    if !calls.is_empty() {
        wire["tool_calls"] = Value::Array(calls);
    }

    wire
}

fn call_to_wire(call: &ToolCall) -> Value {
    json!({
        "id": call.id,
        "type": "function",
        "function": {
            "name": call.name,

            // The format carries arguments as a JSON string, not an object.
            "arguments": call.input.to_string(),
        },
    })
}

fn user_to_wire(message: &Message) -> Vec<Value> {
    let mut wire = Vec::new();

    for block in &message.blocks {
        if let Block::ToolResult(result) = block {
            wire.push(result_to_wire(result));
        }
    }

    let text = message.text();

    if !text.is_empty() {
        wire.push(json!({ "role": "user", "content": text }));
    }

    wire
}

fn result_to_wire(result: &ToolResult) -> Value {
    // There is no error flag in this format, so say it in the text.
    let content = if result.is_error {
        format!("ERROR: {}", result.output)
    } else {
        result.output.clone()
    };

    json!({
        "role": "tool",
        "tool_call_id": result.call_id,
        "content": content,
    })
}

pub fn parse_response(
    json: &Value,
    tool_names: &[&str],
    mut fresh_id: impl FnMut() -> String,
) -> Result<ModelResponse, ProviderError> {
    let choice = json
        .pointer("/choices/0")
        .ok_or_else(|| decode_error("the response has no choices"))?;

    let wire_message = choice
        .get("message")
        .ok_or_else(|| decode_error("the first choice has no message"))?;

    let mut text = wire_message
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let mut calls = structured_calls(wire_message, &mut fresh_id);

    // Small models sometimes write a tool call into their text instead of
    // using the structured field. Recover it rather than ending the run.
    if calls.is_empty() {
        let recovered = text_tool_calls::extract(&text, tool_names);

        if !recovered.calls.is_empty() {
            text = recovered.remaining_text;

            calls = recovered
                .calls
                .into_iter()
                .map(|(name, input)| ToolCall {
                    id: fresh_id(),
                    name,
                    input,
                })
                .collect();
        }
    }

    let stop_reason = if !calls.is_empty() {
        StopReason::ToolUse
    } else {
        parse_finish_reason(choice.get("finish_reason").and_then(Value::as_str))
    };

    let mut blocks = Vec::new();

    if !text.trim().is_empty() {
        blocks.push(Block::Text { text });
    }

    blocks.extend(calls.into_iter().map(Block::ToolCall));

    Ok(ModelResponse {
        message: Message {
            role: Role::Assistant,
            blocks,
        },
        stop_reason,
        usage: parse_usage(json.get("usage")),
    })
}

fn structured_calls(wire_message: &Value, fresh_id: &mut impl FnMut() -> String) -> Vec<ToolCall> {
    let Some(wire_calls) = wire_message.get("tool_calls").and_then(Value::as_array) else {
        return Vec::new();
    };

    wire_calls
        .iter()
        .filter_map(|call| {
            let function = call.get("function")?;
            let name = function.get("name")?.as_str()?.to_string();

            let id = call
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(str::to_string)
                .unwrap_or_else(&mut *fresh_id);

            Some(ToolCall {
                id,
                name,
                input: parse_arguments(function.get("arguments")),
            })
        })
        .collect()
}

/// Arguments normally arrive as a JSON string; some servers send an object.
/// Unparseable arguments are passed through so the tool reports the problem
/// to the model, which can then correct itself.
fn parse_arguments(arguments: Option<&Value>) -> Value {
    match arguments {
        Some(Value::String(text)) => {
            serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.clone()))
        }

        Some(other) => other.clone(),

        None => json!({}),
    }
}

fn parse_finish_reason(reason: Option<&str>) -> StopReason {
    match reason {
        Some("stop") => StopReason::EndTurn,
        Some("tool_calls") => StopReason::ToolUse,
        Some("length") => StopReason::MaxTokens,
        Some("content_filter") => StopReason::Refusal,
        Some(other) => StopReason::Other(other.to_string()),
        None => StopReason::Other("missing".to_string()),
    }
}

fn parse_usage(usage: Option<&Value>) -> Usage {
    let number = |pointer: &str| {
        usage
            .and_then(|usage| usage.pointer(pointer))
            .and_then(Value::as_u64)
            .unwrap_or_default()
    };

    let prompt = number("/prompt_tokens");
    let cached = number("/prompt_tokens_details/cached_tokens");

    Usage {
        input_tokens: prompt.saturating_sub(cached),
        output_tokens: number("/completion_tokens"),
        cache_read_tokens: cached,
        cache_write_tokens: 0,
    }
}

fn decode_error(message: &str) -> ProviderError {
    ProviderError::Decode(message.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> impl FnMut() -> String {
        let mut next = 0;

        move || {
            next += 1;

            format!("generated_{next}")
        }
    }

    #[test]
    fn tool_results_become_tool_messages() {
        let messages = vec![
            Message::user_text("fix it"),
            Message {
                role: Role::Assistant,
                blocks: vec![Block::ToolCall(ToolCall {
                    id: "call_1".to_string(),
                    name: "read_file".to_string(),
                    input: json!({ "path": "a.py" }),
                })],
            },
            Message::tool_results(vec![ToolResult {
                call_id: "call_1".to_string(),
                output: "no such file".to_string(),
                is_error: true,
            }]),
        ];

        let request = ModelRequest {
            system: "be helpful",
            messages: &messages,
            tools: &[],
        };

        let config = OpenAiCompatConfig::local("test-model", None);

        let body = build_request_body(&config, &request);
        let wire = body["messages"].as_array().unwrap();

        assert_eq!(wire.len(), 4);
        assert_eq!(wire[0]["role"], "system");

        assert_eq!(
            wire[2]["tool_calls"][0]["function"]["arguments"],
            r#"{"path":"a.py"}"#
        );

        assert_eq!(wire[3]["role"], "tool");
        assert_eq!(wire[3]["tool_call_id"], "call_1");
        assert_eq!(wire[3]["content"], "ERROR: no such file");
    }

    #[test]
    fn structured_tool_calls_are_parsed() {
        let json = json!({
            "choices": [{
                "finish_reason": "stop",
                "message": {
                    "content": "",
                    "tool_calls": [{
                        "id": "",
                        "function": { "name": "read_file", "arguments": "{\"path\":\"a.py\"}" },
                    }],
                },
            }],
            "usage": { "prompt_tokens": 12, "completion_tokens": 7 },
        });

        let response = parse_response(&json, &["read_file"], ids()).unwrap();

        let calls = response.message.tool_calls();

        // A tool call wins over a `stop` finish reason, and a missing id is
        // filled in.
        assert_eq!(response.stop_reason, StopReason::ToolUse);
        assert_eq!(calls[0].id, "generated_1");
        assert_eq!(calls[0].input, json!({ "path": "a.py" }));
        assert_eq!(response.usage.input_tokens, 12);
        assert_eq!(response.usage.output_tokens, 7);
    }

    #[test]
    fn a_tool_call_written_as_text_is_recovered() {
        let json = json!({
            "choices": [{
                "finish_reason": "stop",
                "message": {
                    "content": "```json\n{\"name\": \"read_file\", \"arguments\": {\"path\": \"a.py\"}}\n```",
                },
            }],
        });

        let response = parse_response(&json, &["read_file"], ids()).unwrap();

        assert_eq!(response.stop_reason, StopReason::ToolUse);
        assert_eq!(response.message.tool_calls()[0].name, "read_file");
        assert_eq!(response.message.text(), "");
    }

    #[test]
    fn plain_text_ends_the_turn() {
        let json = json!({
            "choices": [{
                "finish_reason": "stop",
                "message": { "content": "All done." },
            }],
        });

        let response = parse_response(&json, &["read_file"], ids()).unwrap();

        assert_eq!(response.stop_reason, StopReason::EndTurn);
        assert_eq!(response.message.text(), "All done.");
    }
}
