//! What the window shows about a conversation, built from the agent's
//! events. Replaying a saved trajectory through the same code is how past
//! sessions are shown. The conversation itself lives in `cortex-remote`,
//! so that a phone builds the very same one from the same events.

pub mod history;

use std::borrow::Cow;

use cortex_core::Event;
pub use cortex_remote::stats::{Cell, Stats};
pub use cortex_remote::transcript::{Activity, Doing, Item, Session, Tone, ToolItem, Writing};
use serde_json::Value;

/// Longest tool output kept for showing, in characters. A command can
/// print megabytes; nobody reads them in a card, and a conversation of
/// hundreds of calls would hold them all for as long as it is open.
pub const MAX_OUTPUT_CHARS: usize = 6_000;

/// Longest single piece of text kept from a call's input: the whole of a
/// file being written, say.
const MAX_INPUT_CHARS: usize = 6_000;

/// How a cut text ends, after the number of characters left out.
const CUT_MARK: &str = " more characters";

/// Cuts down what `event` has just added to `session` to what the window
/// shows of it. The engine and the session's log keep the whole; this is
/// only the window's copy.
pub fn slim_after(session: &mut Session, event: &Event) {
    match event {
        Event::ToolStarted { .. } => {
            if let Some(Item::Tool(tool)) = session.items.last_mut() {
                slim_tool(tool);
            }
        }

        Event::ToolFinished { result, .. } => {
            let finished = session.items.iter_mut().rev().find_map(|item| match item {
                Item::Tool(tool) if tool.call.id == result.call_id => Some(tool),
                _ => None,
            });

            if let Some(tool) = finished {
                slim_tool(tool);
            }
        }

        _ => {}
    }
}

/// As [`slim_after`], for every call of a session read back from its log.
pub fn slim(session: &mut Session) {
    for item in &mut session.items {
        if let Item::Tool(tool) = item {
            slim_tool(tool);
        }
    }
}

fn slim_tool(tool: &mut ToolItem) {
    slim_value(&mut tool.call.input);

    let Some(result) = &mut tool.result else {
        return;
    };

    if let Cow::Owned(cut) = clip(&result.output, MAX_OUTPUT_CHARS) {
        result.output = cut;
    }

    // A screenshot is for the model to look at; the card does not draw
    // it, and each is megabytes of text.
    result.images = Vec::new();
}

/// Cuts every long piece of text inside `value`.
fn slim_value(value: &mut Value) {
    match value {
        Value::String(text) => {
            if let Cow::Owned(cut) = clip(text, MAX_INPUT_CHARS) {
                *text = cut;
            }
        }

        Value::Array(values) => values.iter_mut().for_each(slim_value),
        Value::Object(fields) => fields.values_mut().for_each(slim_value),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

/// `text` cut to `max_chars`, with a line saying how much was left out.
/// A text that was cut before is left as it is, so cutting twice does
/// not lose the count.
pub fn clip(text: &str, max_chars: usize) -> Cow<'_, str> {
    // No character takes fewer bytes than one.
    if text.len() <= max_chars {
        return Cow::Borrowed(text);
    }

    let Some((cut, _)) = text.char_indices().nth(max_chars) else {
        return Cow::Borrowed(text);
    };

    let rest = &text[cut..];

    if rest.ends_with(CUT_MARK) && rest.starts_with("\n\n… ") && rest.len() < 64 {
        return Cow::Borrowed(text);
    }

    let hidden = rest.chars().count();

    Cow::Owned(format!("{}\n\n… {hidden}{CUT_MARK}", &text[..cut]))
}

#[cfg(test)]
mod tests {
    use cortex_core::{ToolCall, ToolResult};
    use serde_json::json;

    use super::*;

    fn started(id: &str, input: Value) -> Event {
        Event::ToolStarted {
            step: 1,
            call: ToolCall {
                id: id.to_string(),
                name: "shell".to_string(),
                input,
            },
        }
    }

    fn finished(id: &str, output: String) -> Event {
        Event::ToolFinished {
            step: 1,
            tool: "shell".to_string(),
            result: ToolResult {
                call_id: id.to_string(),
                output,
                is_error: false,
                images: vec![cortex_core::Image {
                    media_type: "image/png".to_string(),
                    data: "x".repeat(50_000),
                }],
            },
            duration_ms: 5,
        }
    }

    fn feed(session: &mut Session, event: Event) {
        session.apply(&event);

        slim_after(session, &event);
    }

    #[test]
    fn a_long_output_is_cut_once_and_keeps_its_count() {
        let long = "é".repeat(MAX_OUTPUT_CHARS + 250);

        let cut = clip(&long, MAX_OUTPUT_CHARS).into_owned();

        assert!(cut.ends_with("… 250 more characters"));
        assert_eq!(
            cut.chars().take(MAX_OUTPUT_CHARS).collect::<String>(),
            "é".repeat(MAX_OUTPUT_CHARS)
        );

        // Cut again, it is what it was.
        assert!(matches!(clip(&cut, MAX_OUTPUT_CHARS), Cow::Borrowed(_)));
        assert!(matches!(clip("short", MAX_OUTPUT_CHARS), Cow::Borrowed(_)));
    }

    #[test]
    fn the_window_does_not_hold_on_to_megabytes_of_tool_output() {
        let mut session = Session::default();

        for call in 0..40 {
            let id = format!("call-{call}");

            feed(
                &mut session,
                started(
                    &id,
                    json!({ "path": "a.rs", "content": "y".repeat(400_000) }),
                ),
            );

            feed(&mut session, finished(&id, "z".repeat(2_000_000)));
        }

        let held: usize = session
            .items
            .iter()
            .map(|item| match item {
                Item::Tool(tool) => {
                    let result = tool.result.as_ref().unwrap();

                    assert!(result.images.is_empty());
                    assert!(result.output.ends_with(CUT_MARK));

                    result.output.len() + tool.call.input.to_string().len()
                }

                _ => 0,
            })
            .sum();

        // Forty calls of 2.4 MB each came in; a few thousand characters of
        // each are kept.
        assert_eq!(session.items.len(), 40);
        assert!(held < 40 * 3 * MAX_OUTPUT_CHARS, "{held} bytes held");
    }

    #[test]
    fn a_short_call_is_kept_whole() {
        let mut session = Session::default();

        feed(
            &mut session,
            started("a", json!({ "command": "ls", "depth": 2 })),
        );
        feed(&mut session, finished("a", "a.rs\nb.rs".to_string()));

        let Item::Tool(tool) = &session.items[0] else {
            panic!("no call");
        };

        assert_eq!(tool.call.input, json!({ "command": "ls", "depth": 2 }));
        assert_eq!(tool.result.as_ref().unwrap().output, "a.rs\nb.rs");
    }
}
