//! Conversion between Cortex types and the Responses format the Codex
//! service speaks.
//!
//! A conversation in this format is a flat list of items: messages, tool
//! calls, tool results, and the model's reasoning. The service keeps
//! nothing between requests, so the whole list is sent each time, and the
//! reasoning travels as an encrypted item that is handed back unchanged.

use cortex_core::{
    Block, Message, ModelRequest, ModelResponse, ProviderError, Role, StopReason, ToolCall,
    ToolResult, ToolSpec, Usage,
};
use serde_json::{Value, json};

use super::CodexConfig;

pub fn build_request_body(
    config: &CodexConfig,
    request: &ModelRequest<'_>,
    session: &str,
) -> Value {
    let input: Vec<Value> = request.messages.iter().flat_map(message_to_wire).collect();
    let tools: Vec<Value> = request.tools.iter().map(tool_to_wire).collect();

    json!({
        "model": config.model,
        "instructions": request.system,
        "input": input,
        "tools": tools,
        "tool_choice": "auto",
        "parallel_tool_calls": true,
        "reasoning": { "effort": config.wire_effort(), "summary": "auto" },

        // The service only answers as a stream, and only without storing.
        "stream": true,
        "store": false,
        "include": ["reasoning.encrypted_content"],

        // Requests with the same key are served from the same cache.
        "prompt_cache_key": session,
    })
}

fn tool_to_wire(spec: &ToolSpec) -> Value {
    json!({
        "type": "function",
        "name": spec.name,
        "description": spec.description,
        "parameters": spec.input_schema,
        "strict": false,
    })
}

fn message_to_wire(message: &Message) -> Vec<Value> {
    let (role, text_kind) = match message.role {
        Role::User => ("user", "input_text"),
        Role::Assistant => ("assistant", "output_text"),
    };

    let mut items = Vec::new();
    let mut texts = Vec::new();

    for block in &message.blocks {
        match block {
            Block::Text { text } if !text.is_empty() => texts.push(json!({
                "type": "message",
                "role": role,
                "content": [{ "type": text_kind, "text": text }],
            })),

            Block::Text { .. } => {}

            Block::ToolCall(call) => items.push(call_to_wire(call)),
            Block::ToolResult(result) => items.push(result_to_wire(result)),

            // Reasoning from this service. Anything else was left by
            // another provider and means nothing here.
            Block::Opaque { raw } if raw["type"] == "reasoning" => items.push(raw.clone()),

            Block::Opaque { .. } => {}
        }
    }

    // A tool result has to follow its call directly, so what the person
    // said alongside the results goes after them. The model's own items
    // stay in the order it wrote them.
    match message.role {
        Role::User => items.extend(texts),
        Role::Assistant => return ordered(message, items, texts),
    }

    items
}

/// Puts an assistant message's items back in the order of its blocks.
fn ordered(message: &Message, items: Vec<Value>, texts: Vec<Value>) -> Vec<Value> {
    let mut items = items.into_iter();
    let mut texts = texts.into_iter();

    message
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Text { text } if text.is_empty() => None,
            Block::Text { .. } => texts.next(),
            Block::Opaque { raw } if raw["type"] != "reasoning" => None,
            _ => items.next(),
        })
        .collect()
}

fn call_to_wire(call: &ToolCall) -> Value {
    json!({
        "type": "function_call",
        "call_id": call.id,
        "name": call.name,

        // The format carries arguments as a JSON string, not an object.
        "arguments": call.input.to_string(),
    })
}

fn result_to_wire(result: &ToolResult) -> Value {
    // There is no error flag in this format, so say it in the text.
    let output = if result.is_error {
        format!("ERROR: {}", result.output)
    } else {
        result.output.clone()
    };

    json!({
        "type": "function_call_output",
        "call_id": result.call_id,
        "output": output,
    })
}

/// What one event of a stream added, for whoever is watching.
#[derive(Debug, Clone, PartialEq)]
pub enum Piece {
    Text(String),
    Thinking(String),
    Calling(String),
}

/// Collects the events of one response.
#[derive(Default)]
pub struct Assembler {
    /// The finished items of the response, in the order they were made.
    items: Vec<Value>,

    /// The closing event's summary: token counts and how it ended.
    summary: Option<Value>,

    failure: Option<String>,
}

impl Assembler {
    /// Takes one line of the stream and returns what it added.
    pub fn push_line(&mut self, line: &str) -> Option<Piece> {
        // Blank lines separate events, `event:` lines repeat the type
        // that is in the data, and lines starting with a colon are
        // keep-alive comments.
        let data = line.strip_prefix("data:")?.trim();

        // An event that cannot be read is skipped rather than ending the
        // response: the rest of the stream is still worth having.
        let event: Value = serde_json::from_str(data).ok()?;

        let text = |pointer: &str| {
            event
                .pointer(pointer)
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .map(str::to_string)
        };

        match event["type"].as_str()? {
            "response.output_text.delta" => text("/delta").map(Piece::Text),

            "response.reasoning_summary_text.delta" => text("/delta").map(Piece::Thinking),

            // The summary comes in parts, each a paragraph of its own.
            "response.reasoning_summary_part.added" => {
                (event["summary_index"].as_u64()? > 0).then(|| Piece::Thinking("\n\n".to_string()))
            }

            "response.output_item.added" => (event["item"]["type"] == "function_call")
                .then(|| text("/item/name").map(Piece::Calling))?,

            "response.output_item.done" => {
                self.items.push(event["item"].clone());

                None
            }

            "response.completed" | "response.incomplete" => {
                self.summary = Some(event["response"].clone());

                None
            }

            "response.failed" | "error" => {
                let message = text("/response/error/message")
                    .or_else(|| text("/error/message"))
                    .or_else(|| text("/message"))
                    .unwrap_or_else(|| data.to_string());

                self.failure = Some(message);

                None
            }

            _ => None,
        }
    }

    /// Whether the service said the response was over, well or badly.
    pub fn finished(&self) -> bool {
        self.summary.is_some() || self.failure.is_some()
    }

    pub fn into_response(self) -> Result<ModelResponse, ProviderError> {
        if let Some(failure) = self.failure {
            return Err(ProviderError::Failed(failure));
        }

        let summary = self.summary.unwrap_or_default();

        let blocks: Vec<Block> = self.items.iter().filter_map(item_to_block).collect();

        let called = blocks
            .iter()
            .any(|block| matches!(block, Block::ToolCall(_)));

        let stop_reason = match summary
            .pointer("/incomplete_details/reason")
            .and_then(Value::as_str)
        {
            _ if called => StopReason::ToolUse,
            Some("max_output_tokens") => StopReason::MaxTokens,
            Some("content_filter") => StopReason::Refusal,
            Some(other) => StopReason::Other(other.to_string()),
            None => StopReason::EndTurn,
        };

        Ok(ModelResponse {
            message: Message {
                role: Role::Assistant,
                blocks,
            },
            stop_reason,
            usage: parse_usage(&summary["usage"]),
        })
    }
}

fn item_to_block(item: &Value) -> Option<Block> {
    match item["type"].as_str()? {
        "message" => {
            let text: String = item["content"]
                .as_array()?
                .iter()
                .filter_map(|part| part["text"].as_str().or_else(|| part["refusal"].as_str()))
                .collect();

            (!text.trim().is_empty()).then_some(Block::Text { text })
        }

        "function_call" => Some(Block::ToolCall(ToolCall {
            id: item["call_id"].as_str()?.to_string(),
            name: item["name"].as_str()?.to_string(),
            input: parse_arguments(&item["arguments"]),
        })),

        // Reasoning is only worth keeping in the form that can be sent
        // back. Its id names a stored copy, and nothing is stored.
        "reasoning" if item["encrypted_content"].is_string() => {
            let mut raw = item.clone();

            raw.as_object_mut()?.remove("id");

            Some(Block::Opaque { raw })
        }

        _ => None,
    }
}

/// Unparseable arguments are passed through so the tool reports the
/// problem to the model, which can then correct itself.
fn parse_arguments(arguments: &Value) -> Value {
    match arguments {
        Value::String(text) if text.trim().is_empty() => json!({}),

        Value::String(text) => {
            serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.clone()))
        }

        Value::Null => json!({}),

        other => other.clone(),
    }
}

fn parse_usage(usage: &Value) -> Usage {
    let number = |pointer: &str| {
        usage
            .pointer(pointer)
            .and_then(Value::as_u64)
            .unwrap_or_default()
    };

    let input = number("/input_tokens");
    let cached = number("/input_tokens_details/cached_tokens");

    Usage {
        input_tokens: input.saturating_sub(cached),
        output_tokens: number("/output_tokens"),
        cache_read_tokens: cached,
        cache_write_tokens: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assemble(lines: &[&str]) -> (Vec<Piece>, Assembler) {
        let mut assembler = Assembler::default();

        let pieces = lines
            .iter()
            .filter_map(|line| assembler.push_line(line))
            .collect();

        (pieces, assembler)
    }

    #[test]
    fn a_conversation_becomes_a_flat_list_of_items() {
        let messages = vec![
            Message::user_text("fix it"),
            Message {
                role: Role::Assistant,
                blocks: vec![
                    Block::Opaque {
                        raw: json!({ "type": "reasoning", "encrypted_content": "abc" }),
                    },
                    Block::Opaque {
                        raw: json!({ "type": "thinking", "signature": "other provider" }),
                    },
                    Block::Text {
                        text: "Reading the file.".to_string(),
                    },
                    Block::ToolCall(ToolCall {
                        id: "call_1".to_string(),
                        name: "read_file".to_string(),
                        input: json!({ "path": "a.py" }),
                    }),
                ],
            },
            Message {
                role: Role::User,
                blocks: vec![
                    Block::Text {
                        text: "also check b.py".to_string(),
                    },
                    Block::ToolResult(ToolResult {
                        call_id: "call_1".to_string(),
                        output: "no such file".to_string(),
                        is_error: true,
                    }),
                ],
            },
        ];

        let request = ModelRequest {
            system: "be helpful",
            messages: &messages,
            tools: &[],
        };

        let body = build_request_body(&CodexConfig::new("test-model", "max"), &request, "session");
        let input = body["input"].as_array().unwrap();

        let kinds: Vec<&str> = input
            .iter()
            .map(|item| item["type"].as_str().unwrap())
            .collect();

        assert_eq!(
            kinds,
            [
                "message",
                "reasoning",
                "message",
                "function_call",
                "function_call_output",
                "message"
            ]
        );

        assert_eq!(body["instructions"], "be helpful");
        assert_eq!(body["reasoning"]["effort"], "xhigh");
        assert_eq!(input[2]["content"][0]["type"], "output_text");
        assert_eq!(input[3]["arguments"], r#"{"path":"a.py"}"#);
        assert_eq!(input[4]["output"], "ERROR: no such file");
        assert_eq!(input[5]["content"][0]["text"], "also check b.py");
    }

    #[test]
    fn a_streamed_response_is_put_back_together() {
        let (pieces, assembler) = assemble(&[
            "event: response.created",
            r#"data: {"type":"response.created","response":{}}"#,
            r#"data: {"type":"response.reasoning_summary_text.delta","delta":"Look first."}"#,
            r#"data: {"type":"response.output_item.done","item":{"type":"reasoning","id":"rs_1","summary":[],"encrypted_content":"abc"}}"#,
            r#"data: {"type":"response.output_text.delta","delta":"Read"}"#,
            r#"data: {"type":"response.output_text.delta","delta":"ing."}"#,
            r#"data: {"type":"response.output_item.done","item":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Reading."}]}}"#,
            r#"data: {"type":"response.output_item.added","item":{"type":"function_call","name":"read_file","arguments":""}}"#,
            r#"data: {"type":"response.output_item.done","item":{"type":"function_call","call_id":"call_1","name":"read_file","arguments":"{\"path\":\"a.py\"}"}}"#,
            r#"data: {"type":"response.completed","response":{"usage":{"input_tokens":100,"input_tokens_details":{"cached_tokens":80},"output_tokens":7}}}"#,
        ]);

        assert_eq!(
            pieces,
            [
                Piece::Thinking("Look first.".to_string()),
                Piece::Text("Read".to_string()),
                Piece::Text("ing.".to_string()),
                Piece::Calling("read_file".to_string()),
            ]
        );

        assert!(assembler.finished());

        let response = assembler.into_response().unwrap();

        assert_eq!(response.stop_reason, StopReason::ToolUse);
        assert_eq!(response.message.text(), "Reading.");
        assert_eq!(
            response.message.tool_calls()[0].input,
            json!({ "path": "a.py" })
        );

        assert_eq!(
            response.message.blocks[0],
            Block::Opaque {
                raw: json!({ "type": "reasoning", "summary": [], "encrypted_content": "abc" }),
            }
        );

        assert_eq!(response.usage.input_tokens, 20);
        assert_eq!(response.usage.cache_read_tokens, 80);
        assert_eq!(response.usage.output_tokens, 7);
    }

    #[test]
    fn a_stream_cut_short_is_not_finished() {
        let (_, assembler) =
            assemble(&[r#"data: {"type":"response.output_text.delta","delta":"Hel"}"#]);

        assert!(!assembler.finished());
    }

    #[test]
    fn a_failed_response_is_an_error() {
        let (_, assembler) = assemble(&[
            r#"data: {"type":"response.failed","response":{"error":{"message":"too long"}}}"#,
        ]);

        assert!(assembler.finished());

        assert!(matches!(
            assembler.into_response(),
            Err(ProviderError::Failed(message)) if message == "too long"
        ));
    }
}
