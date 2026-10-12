//! Recovering tool calls that a model wrote as text.
//!
//! Small local models do not always use the structured tool-call field.
//! Instead they print the call, in one of three common shapes:
//!
//! - `<tool_call>{"name": ..., "arguments": {...}}</tool_call>`
//! - a fenced code block containing that JSON object
//! - the bare JSON object as the whole reply
//!
//! Only objects naming a tool that was actually offered are accepted, so
//! ordinary JSON in an answer is left alone.

use serde_json::Value;

const OPEN_TAG: &str = "<tool_call>";
const CLOSE_TAG: &str = "</tool_call>";

#[derive(Debug, Default)]
pub struct Extracted {
    /// `(tool name, arguments)` for each recovered call.
    pub calls: Vec<(String, Value)>,

    /// The reply with the recovered calls removed.
    pub remaining_text: String,
}

pub fn extract(text: &str, tool_names: &[&str]) -> Extracted {
    let tagged = extract_tagged(text, tool_names);

    if !tagged.calls.is_empty() {
        return tagged;
    }

    extract_whole_reply(text, tool_names)
}

/// Finds every `<tool_call>...</tool_call>` section.
fn extract_tagged(text: &str, tool_names: &[&str]) -> Extracted {
    let mut calls = Vec::new();
    let mut remaining_text = String::new();
    let mut rest = text;

    while let Some(start) = rest.find(OPEN_TAG) {
        let after_open = &rest[start + OPEN_TAG.len()..];

        // A missing closing tag means the call runs to the end of the reply.
        let (inner, after_close) = match after_open.find(CLOSE_TAG) {
            Some(end) => (&after_open[..end], &after_open[end + CLOSE_TAG.len()..]),
            None => (after_open, ""),
        };

        remaining_text.push_str(&rest[..start]);

        match parse_call(inner, tool_names) {
            Some(call) => calls.push(call),

            // Not a usable call: keep the text so nothing is silently lost.
            None => remaining_text.push_str(&rest[start..rest.len() - after_close.len()]),
        }

        rest = after_close;
    }

    remaining_text.push_str(rest);

    Extracted {
        calls,
        remaining_text: remaining_text.trim().to_string(),
    }
}

/// Accepts a reply that is nothing but one JSON call, optionally fenced.
fn extract_whole_reply(text: &str, tool_names: &[&str]) -> Extracted {
    let candidate = strip_code_fence(text.trim());

    match parse_call(candidate, tool_names) {
        Some(call) => Extracted {
            calls: vec![call],
            remaining_text: String::new(),
        },

        None => Extracted {
            calls: Vec::new(),
            remaining_text: text.to_string(),
        },
    }
}

fn strip_code_fence(text: &str) -> &str {
    let Some(without_open) = text.strip_prefix("```") else {
        return text;
    };

    let Some(without_close) = without_open.strip_suffix("```") else {
        return text;
    };

    // Drop the language tag (`json`) on the opening line.
    match without_close.split_once('\n') {
        Some((_language, body)) => body.trim(),
        None => without_close.trim(),
    }
}

fn parse_call(text: &str, tool_names: &[&str]) -> Option<(String, Value)> {
    let value: Value = serde_json::from_str(text.trim()).ok()?;

    let name = value.get("name")?.as_str()?;

    if !tool_names.contains(&name) {
        return None;
    }

    let arguments = value
        .get("arguments")
        .or_else(|| value.get("parameters"))
        .cloned()
        .unwrap_or_else(|| Value::Object(Default::default()));

    Some((name.to_string(), arguments))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const TOOLS: [&str; 2] = ["read_file", "shell"];

    #[test]
    fn tagged_calls_are_found_and_removed_from_the_text() {
        let text = "Let me look.\n<tool_call>\n{\"name\": \"read_file\", \"arguments\": {\"path\": \"a.py\"}}\n</tool_call>";

        let extracted = extract(text, &TOOLS);

        assert_eq!(
            extracted.calls,
            vec![("read_file".to_string(), json!({ "path": "a.py" }))]
        );

        assert_eq!(extracted.remaining_text, "Let me look.");
    }

    #[test]
    fn several_tagged_calls_are_all_found() {
        let text = "<tool_call>{\"name\":\"read_file\",\"arguments\":{}}</tool_call>\
                    <tool_call>{\"name\":\"shell\",\"arguments\":{\"command\":\"ls\"}}</tool_call>";

        assert_eq!(extract(text, &TOOLS).calls.len(), 2);
    }

    #[test]
    fn a_bare_json_reply_is_a_call() {
        let text = r#"{"name": "shell", "arguments": {"command": "ls"}}"#;

        let extracted = extract(text, &TOOLS);

        assert_eq!(extracted.calls[0].0, "shell");
        assert!(extracted.remaining_text.is_empty());
    }

    #[test]
    fn unknown_tools_and_ordinary_json_are_left_alone() {
        let unknown = r#"{"name": "launch_rocket", "arguments": {}}"#;
        let ordinary = r#"Here is the config: {"name": "demo"}"#;

        assert!(extract(unknown, &TOOLS).calls.is_empty());
        assert!(extract(ordinary, &TOOLS).calls.is_empty());

        assert_eq!(extract(ordinary, &TOOLS).remaining_text, ordinary);
    }
}
