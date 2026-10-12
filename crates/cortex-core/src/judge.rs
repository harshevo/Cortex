//! Asking the model a question beside the run.
//!
//! The review of finished work, the lessons taken from a run, and the
//! choice between parallel attempts are each one question put to the model
//! outside the conversation: no tools, no history, a structured answer.

use serde_json::Value;

use crate::error::ProviderError;
use crate::message::Message;
use crate::provider::{ModelProvider, ModelRequest};
use crate::usage::Usage;

pub struct Answer {
    pub text: String,
    pub usage: Usage,
}

impl Answer {
    /// The JSON value in the answer, when there is one.
    pub fn json(&self) -> Option<Value> {
        json_in(&self.text)
    }
}

pub async fn ask(
    provider: &dyn ModelProvider,
    system: &str,
    question: &str,
) -> Result<Answer, ProviderError> {
    let messages = [Message::user_text(question)];

    let request = ModelRequest {
        system,
        messages: &messages,
        tools: &[],
    };

    let response = provider.complete(request).await?;

    Ok(Answer {
        text: response.message.text(),
        usage: response.usage,
    })
}

/// The first JSON object or array in `text`.
///
/// Models wrap JSON in a code fence or put a sentence before it, so the
/// text is searched for the first place a value parses.
pub fn json_in(text: &str) -> Option<Value> {
    for (start, opening) in text.char_indices() {
        if opening != '{' && opening != '[' {
            continue;
        }

        let mut values = serde_json::Deserializer::from_str(&text[start..]).into_iter::<Value>();

        if let Some(Ok(value)) = values.next() {
            return Some(value);
        }
    }

    None
}

/// Shortens `text` to `max_chars`, keeping the start and the end.
pub fn clip_middle(text: &str, max_chars: usize) -> String {
    crate::tool::truncate_middle(text, max_chars)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn json_is_found_inside_a_fence_and_after_prose() {
        let text = "Here it is:\n```json\n{\"met\": true, \"why\": \"a {brace}\"}\n```\nDone.";

        assert_eq!(
            json_in(text),
            Some(json!({ "met": true, "why": "a {brace}" }))
        );
    }

    #[test]
    fn text_without_json_gives_nothing() {
        assert_eq!(json_in("no value here { not json"), None);
    }
}
