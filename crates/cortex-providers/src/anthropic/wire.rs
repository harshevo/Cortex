//! Conversion between Cortex types and the Messages API JSON.

use cortex_core::{
    Block, Image, Message, ModelRequest, ModelResponse, ProviderError, Role, StopReason, ToolCall,
    ToolResult, ToolSpec, Usage,
};
use serde_json::{Value, json};

use super::AnthropicConfig;

pub fn build_request_body(config: &AnthropicConfig, request: &ModelRequest<'_>) -> Value {
    let messages: Vec<Value> = request.messages.iter().map(message_to_wire).collect();
    let tools: Vec<Value> = request.tools.iter().map(tool_to_wire).collect();

    // The system prompt is the stable start of every request, so it gets an
    // explicit cache breakpoint. The top-level `cache_control` additionally
    // caches the growing conversation behind it.
    let system = json!([{
        "type": "text",
        "text": request.system,
        "cache_control": { "type": "ephemeral" },
    }]);

    let mut body = json!({
        "model": config.model,
        "max_tokens": config.max_tokens,
        "system": system,
        "messages": messages,
        "output_config": { "effort": config.effort },
        "cache_control": { "type": "ephemeral" },
    });

    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
    }

    if config.refusal_fallback {
        body["fallbacks"] = json!("default");
    }

    body
}

fn tool_to_wire(spec: &ToolSpec) -> Value {
    json!({
        "name": spec.name,
        "description": spec.description,
        "input_schema": spec.input_schema,
    })
}

fn message_to_wire(message: &Message) -> Value {
    let role = match message.role {
        Role::User => "user",
        Role::Assistant => "assistant",
    };

    let content: Vec<Value> = message.blocks.iter().filter_map(block_to_wire).collect();

    json!({ "role": role, "content": content })
}

fn block_to_wire(block: &Block) -> Option<Value> {
    match block {
        // The API rejects empty text blocks.
        Block::Text { text } if text.is_empty() => None,

        Block::Text { text } => Some(json!({ "type": "text", "text": text })),

        Block::ToolCall(call) => Some(json!({
            "type": "tool_use",
            "id": call.id,
            "name": call.name,
            "input": call.input,
        })),

        Block::ToolResult(result) => Some(json!({
            "type": "tool_result",
            "tool_use_id": result.call_id,
            "content": result_content(result),
            "is_error": result.is_error,
        })),

        Block::Image(image) => Some(image_to_wire(image)),

        // Reasoning and other provider blocks go back exactly as received.
        Block::Opaque { raw } => Some(raw.clone()),
    }
}

/// Plain text, or a list when the result carries pictures such as a
/// screenshot.
fn result_content(result: &ToolResult) -> Value {
    if result.images.is_empty() {
        return json!(result.output);
    }

    let mut content = Vec::new();

    if !result.output.is_empty() {
        content.push(json!({ "type": "text", "text": result.output }));
    }

    content.extend(result.images.iter().map(image_to_wire));

    json!(content)
}

fn image_to_wire(image: &Image) -> Value {
    json!({
        "type": "image",
        "source": {
            "type": "base64",
            "media_type": image.media_type,
            "data": image.data,
        },
    })
}

pub fn parse_response(json: &Value) -> Result<ModelResponse, ProviderError> {
    let content = json
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| decode_error("the response has no `content` array"))?;

    let blocks = content
        .iter()
        .map(block_from_wire)
        .collect::<Result<_, _>>()?;

    let message = Message {
        role: Role::Assistant,
        blocks,
    };

    let stop_reason = parse_stop_reason(json.get("stop_reason").and_then(Value::as_str));
    let usage = parse_usage(json.get("usage"));

    Ok(ModelResponse {
        message,
        stop_reason,
        usage,
    })
}

fn block_from_wire(block: &Value) -> Result<Block, ProviderError> {
    let kind = block
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();

    match kind {
        "text" => {
            let text = block
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default();

            Ok(Block::Text {
                text: text.to_string(),
            })
        }

        "tool_use" => {
            let id = required_string(block, "id")?;
            let name = required_string(block, "name")?;
            let input = block.get("input").cloned().unwrap_or_else(|| json!({}));

            Ok(Block::ToolCall(ToolCall { id, name, input }))
        }

        _ => Ok(Block::Opaque { raw: block.clone() }),
    }
}

fn parse_stop_reason(reason: Option<&str>) -> StopReason {
    match reason {
        Some("end_turn") | Some("stop_sequence") => StopReason::EndTurn,
        Some("tool_use") => StopReason::ToolUse,
        Some("max_tokens") => StopReason::MaxTokens,
        Some("refusal") => StopReason::Refusal,
        Some("pause_turn") => StopReason::Paused,
        Some(other) => StopReason::Other(other.to_string()),
        None => StopReason::Other("missing".to_string()),
    }
}

fn parse_usage(usage: Option<&Value>) -> Usage {
    let field = |name: &str| {
        usage
            .and_then(|usage| usage.get(name))
            .and_then(Value::as_u64)
            .unwrap_or_default()
    };

    Usage {
        input_tokens: field("input_tokens"),
        output_tokens: field("output_tokens"),
        cache_read_tokens: field("cache_read_input_tokens"),
        cache_write_tokens: field("cache_creation_input_tokens"),
    }
}

fn required_string(block: &Value, field: &str) -> Result<String, ProviderError> {
    block
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| decode_error(&format!("a tool_use block has no `{field}`")))
}

fn decode_error(message: &str) -> ProviderError {
    ProviderError::Decode(message.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture() -> Message {
        Message::user_with_images(
            "what is this?",
            &[cortex_core::Image {
                media_type: "image/png".to_string(),
                data: "QUJD".to_string(),
            }],
        )
    }

    #[test]
    fn a_picture_is_sent_as_a_base64_image_block() {
        let wire = message_to_wire(&picture());

        assert_eq!(wire["content"][0]["text"], "what is this?");
        assert_eq!(wire["content"][1]["type"], "image");
        assert_eq!(wire["content"][1]["source"]["media_type"], "image/png");
        assert_eq!(wire["content"][1]["source"]["data"], "QUJD");
    }

    #[test]
    fn a_screenshot_travels_inside_its_tool_result() {
        let message = Message::tool_results(vec![ToolResult {
            call_id: "call_1".to_string(),
            output: "saved".to_string(),
            is_error: false,
            images: vec![Image {
                media_type: "image/png".to_string(),
                data: "QUJD".to_string(),
            }],
        }]);

        let wire = message_to_wire(&message);
        let content = &wire["content"][0]["content"];

        assert_eq!(content[0]["text"], "saved");
        assert_eq!(content[1]["type"], "image");
        assert_eq!(content[1]["source"]["data"], "QUJD");
    }

    fn config() -> AnthropicConfig {
        AnthropicConfig {
            api_key: "test".to_string(),
            base_url: "http://localhost".to_string(),
            model: "claude-opus-5-5".to_string(),
            max_tokens: 16_000,
            effort: "high".to_string(),
            refusal_fallback: true,
        }
    }

    #[test]
    fn request_body_has_the_expected_shape() {
        let reasoning = json!({ "type": "thinking", "thinking": "", "signature": "sig" });

        let messages = vec![
            Message::user_text("fix the bug"),
            Message {
                role: Role::Assistant,
                blocks: vec![
                    Block::Opaque {
                        raw: reasoning.clone(),
                    },
                    Block::Text {
                        text: String::new(),
                    },
                    Block::ToolCall(ToolCall {
                        id: "call_1".to_string(),
                        name: "read_file".to_string(),
                        input: json!({ "path": "a.py" }),
                    }),
                ],
            },
            Message::tool_results(vec![ToolResult {
                call_id: "call_1".to_string(),
                output: "print(1)".to_string(),
                is_error: false,
                images: Vec::new(),
            }]),
        ];

        let request = ModelRequest {
            system: "be helpful",
            messages: &messages,
            tools: &[],
        };

        let body = build_request_body(&config(), &request);

        assert_eq!(body["model"], "claude-opus-5-5");
        assert_eq!(body["output_config"]["effort"], "high");
        assert_eq!(body["fallbacks"], "default");
        assert_eq!(body["system"][0]["text"], "be helpful");
        assert!(body.get("tools").is_none());

        // The reasoning block is replayed verbatim and the empty text block
        // is dropped.
        let assistant = body["messages"][1]["content"].as_array().unwrap();

        assert_eq!(assistant.len(), 2);
        assert_eq!(assistant[0], reasoning);
        assert_eq!(assistant[1]["type"], "tool_use");

        let result = &body["messages"][2]["content"][0];

        assert_eq!(result["type"], "tool_result");
        assert_eq!(result["tool_use_id"], "call_1");
    }

    #[test]
    fn response_is_parsed_into_blocks_and_usage() {
        let json = json!({
            "content": [
                { "type": "thinking", "thinking": "", "signature": "sig" },
                { "type": "text", "text": "Reading the file." },
                { "type": "tool_use", "id": "call_1", "name": "read_file", "input": { "path": "a.py" } },
            ],
            "stop_reason": "tool_use",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 20,
                "cache_read_input_tokens": 30,
                "cache_creation_input_tokens": 40,
            },
        });

        let response = parse_response(&json).unwrap();

        assert_eq!(response.stop_reason, StopReason::ToolUse);
        assert_eq!(response.message.text(), "Reading the file.");
        assert_eq!(response.message.tool_calls().len(), 1);
        assert!(matches!(response.message.blocks[0], Block::Opaque { .. }));

        assert_eq!(response.usage.input_tokens, 10);
        assert_eq!(response.usage.output_tokens, 20);
        assert_eq!(response.usage.cache_read_tokens, 30);
        assert_eq!(response.usage.cache_write_tokens, 40);
    }
}
