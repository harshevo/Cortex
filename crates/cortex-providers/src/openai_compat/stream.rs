//! Reading a response that arrives piece by piece.
//!
//! With `"stream": true` the server sends the response as a series of
//! small JSON chunks, each on a line beginning `data: `. This module puts
//! the chunks back together into the same JSON a non-streamed request
//! returns, so everything after it is shared with the ordinary path.

use serde_json::{Value, json};

/// The line that ends a stream.
const DONE: &str = "[DONE]";

/// Splits the bytes of a stream into complete lines. Network reads end
/// wherever they like, so a line may arrive in several parts.
#[derive(Default)]
pub struct Lines {
    unfinished: Vec<u8>,
}

impl Lines {
    /// Takes the next bytes and returns the lines they complete.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.unfinished.extend_from_slice(bytes);

        let mut lines = Vec::new();

        while let Some(end) = self.unfinished.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.unfinished.drain(..=end).collect();

            lines.push(String::from_utf8_lossy(&line).trim().to_string());
        }

        lines
    }
}

/// One tool call, built up from the pieces of its name and arguments.
#[derive(Default)]
struct CallParts {
    id: String,
    name: String,
    arguments: String,
}

/// What one line of a stream added, for whoever is watching.
#[derive(Debug, Clone, PartialEq)]
pub enum Piece {
    /// More of the model's answer.
    Text(String),

    /// More of its reasoning. Servers name the field differently:
    /// `reasoning_content` (DeepSeek and those that follow it) or
    /// `reasoning` (OpenRouter and Ollama).
    Thinking(String),

    /// The model has begun asking for the tool of this name.
    Calling(String),
}

/// Collects the chunks of one response.
#[derive(Default)]
pub struct Assembler {
    content: String,
    calls: Vec<CallParts>,
    finish_reason: Option<String>,
    usage: Option<Value>,
    finished: bool,
}

impl Assembler {
    /// Takes one line of the stream. Returns what it added to the
    /// response. Reasoning is only passed on, not kept: the response is
    /// the same as one that was not streamed.
    pub fn push_line(&mut self, line: &str) -> Vec<Piece> {
        self.take_line(line).unwrap_or_default()
    }

    fn take_line(&mut self, line: &str) -> Option<Vec<Piece>> {
        // Blank lines separate chunks, and lines starting with a colon
        // are keep-alive comments.
        let data = line.strip_prefix("data:")?.trim();

        if data == DONE {
            self.finished = true;

            return None;
        }

        // A chunk that cannot be read is skipped rather than ending the
        // response: the rest of the stream is still worth having.
        let chunk: Value = serde_json::from_str(data).ok()?;

        if let Some(usage) = chunk.get("usage").filter(|usage| usage.is_object()) {
            self.usage = Some(usage.clone());
        }

        let choice = chunk.pointer("/choices/0")?;

        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.finish_reason = Some(reason.to_string());
        }

        let delta = choice.get("delta")?;

        let mut pieces = Vec::new();

        for call in delta
            .get("tool_calls")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(name) = self.push_call(call) {
                pieces.push(Piece::Calling(name));
            }
        }

        let field = |name: &str| {
            delta
                .get(name)
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
        };

        if let Some(thinking) = field("reasoning_content").or_else(|| field("reasoning")) {
            pieces.push(Piece::Thinking(thinking.to_string()));
        }

        if let Some(text) = field("content") {
            self.content.push_str(text);

            pieces.push(Piece::Text(text.to_string()));
        }

        Some(pieces)
    }

    /// The first piece of a call carries its id and name; the pieces
    /// after it carry the arguments, a few characters at a time. Returns
    /// the name of a call that has just begun.
    fn push_call(&mut self, piece: &Value) -> Option<String> {
        let index = piece
            .get("index")
            .and_then(Value::as_u64)
            .unwrap_or(self.calls.len().saturating_sub(1) as u64) as usize;

        if self.calls.len() <= index {
            self.calls.resize_with(index + 1, CallParts::default);
        }

        let call = &mut self.calls[index];

        let text = |pointer: &str| piece.pointer(pointer).and_then(Value::as_str);

        if let Some(id) = text("/id") {
            call.id.push_str(id);
        }

        let unnamed = call.name.is_empty();

        if let Some(name) = text("/function/name") {
            call.name.push_str(name);
        }

        if let Some(arguments) = text("/function/arguments") {
            call.arguments.push_str(arguments);
        }

        (unnamed && !call.name.is_empty()).then(|| call.name.clone())
    }

    /// Whether the server said the response was complete.
    pub fn finished(&self) -> bool {
        self.finished || self.finish_reason.is_some()
    }

    /// The response, in the shape a non-streamed request returns.
    pub fn into_response(self) -> Value {
        let mut message = json!({
            "role": "assistant",
            "content": self.content,
        });

        if !self.calls.is_empty() {
            let calls: Vec<Value> = self
                .calls
                .into_iter()
                .map(|call| {
                    json!({
                        "id": call.id,
                        "type": "function",
                        "function": { "name": call.name, "arguments": call.arguments },
                    })
                })
                .collect();

            message["tool_calls"] = Value::Array(calls);
        }

        let mut response = json!({
            "choices": [{ "message": message, "finish_reason": self.finish_reason }],
        });

        if let Some(usage) = self.usage {
            response["usage"] = usage;
        }

        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assemble(lines: &[&str]) -> (Vec<String>, Value) {
        let mut assembler = Assembler::default();

        let pieces = lines
            .iter()
            .flat_map(|line| assembler.push_line(line))
            .filter_map(|piece| match piece {
                Piece::Text(text) => Some(text),
                Piece::Thinking(_) | Piece::Calling(_) => None,
            })
            .collect();

        (pieces, assembler.into_response())
    }

    #[test]
    fn lines_split_across_reads_are_joined() {
        let mut lines = Lines::default();

        assert!(lines.push(b"data: {\"a\"").is_empty());
        assert_eq!(lines.push(b": 1}\r\n\ndata: x"), ["data: {\"a\": 1}", ""]);
        assert_eq!(lines.push(b"\n"), ["data: x"]);
    }

    #[test]
    fn text_arrives_in_pieces_and_ends_whole() {
        let (pieces, response) = assemble(&[
            r#"data: {"choices":[{"delta":{"role":"assistant","content":""}}]}"#,
            r#"data: {"choices":[{"delta":{"content":"Hel"}}]}"#,
            ": keep-alive",
            "",
            r#"data: {"choices":[{"delta":{"content":"lo"},"finish_reason":"stop"}]}"#,
            r#"data: {"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":2}}"#,
            "data: [DONE]",
        ]);

        assert_eq!(pieces, ["Hel", "lo"]);
        assert_eq!(response["choices"][0]["message"]["content"], "Hello");
        assert_eq!(response["choices"][0]["finish_reason"], "stop");
        assert_eq!(response["usage"]["completion_tokens"], 2);
    }

    #[test]
    fn reasoning_is_passed_on_and_left_out_of_the_response() {
        let mut assembler = Assembler::default();

        let lines = [
            r#"data: {"choices":[{"delta":{"reasoning_content":"The loop "}}]}"#,
            r#"data: {"choices":[{"delta":{"reasoning":"stops early."}}]}"#,
            r#"data: {"choices":[{"delta":{"content":"Fixed."},"finish_reason":"stop"}]}"#,
        ];

        let pieces: Vec<Piece> = lines
            .iter()
            .flat_map(|line| assembler.push_line(line))
            .collect();

        assert_eq!(
            pieces,
            [
                Piece::Thinking("The loop ".to_string()),
                Piece::Thinking("stops early.".to_string()),
                Piece::Text("Fixed.".to_string()),
            ]
        );

        assert_eq!(
            assembler.into_response()["choices"][0]["message"]["content"],
            "Fixed."
        );
    }

    #[test]
    fn a_tool_call_is_put_back_together() {
        let (pieces, response) = assemble(&[
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"read_file","arguments":""}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"path\":"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"a.py\"}"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":1,"id":"call_2","function":{"name":"search","arguments":"{}"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
        ]);

        assert!(pieces.is_empty());

        let calls = &response["choices"][0]["message"]["tool_calls"];

        assert_eq!(calls[0]["id"], "call_1");
        assert_eq!(calls[0]["function"]["name"], "read_file");
        assert_eq!(calls[0]["function"]["arguments"], r#"{"path":"a.py"}"#);
        assert_eq!(calls[1]["function"]["name"], "search");
        assert_eq!(response["choices"][0]["finish_reason"], "tool_calls");
    }

    #[test]
    fn the_start_of_each_tool_call_is_announced_once() {
        let mut assembler = Assembler::default();

        let pieces: Vec<Piece> = [
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"read_file","arguments":""}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{}"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":1,"id":"call_2","function":{"name":"search","arguments":"{}"}}]}}]}"#,
        ]
        .iter()
        .flat_map(|line| assembler.push_line(line))
        .collect();

        assert_eq!(
            pieces,
            [
                Piece::Calling("read_file".to_string()),
                Piece::Calling("search".to_string()),
            ]
        );
    }

    #[test]
    fn a_stream_cut_short_is_not_finished() {
        let mut assembler = Assembler::default();

        assembler.push_line(r#"data: {"choices":[{"delta":{"content":"Hel"}}]}"#);

        assert!(!assembler.finished());
    }
}
