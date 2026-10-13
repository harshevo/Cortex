//! Provider-neutral conversation types.
//!
//! Every provider converts these to and from its own wire format, so the
//! agent loop never depends on one vendor's JSON shape.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
}

/// A request from the model to run one tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
}

/// What a tool produced for one [`ToolCall`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    pub call_id: String,
    pub output: String,
    pub is_error: bool,

    /// Pictures that belong to the output, such as a screenshot, for the
    /// model to look at.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<Image>,
}

/// A picture: one the person attached to an instruction, or one a tool
/// returned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Image {
    /// Such as `image/png`.
    pub media_type: String,

    /// The file's bytes in base64.
    pub data: String,
}

impl Image {
    /// The picture as a `data:` address, the form two of the provider
    /// formats take it in.
    pub fn data_url(&self) -> String {
        format!("data:{};base64,{}", self.media_type, self.data)
    }
}

/// One piece of a message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Block {
    Text {
        text: String,
    },

    ToolCall(ToolCall),

    ToolResult(ToolResult),

    Image(Image),

    /// A provider-specific block (for example model reasoning) that the core
    /// does not interpret but must send back unchanged on the next request.
    Opaque {
        raw: Value,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub blocks: Vec<Block>,
}

impl Message {
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            blocks: vec![Block::Text { text: text.into() }],
        }
    }

    /// What the person said, followed by the pictures sent with it.
    pub fn user_with_images(text: impl Into<String>, images: &[Image]) -> Self {
        let mut message = Self::user_text(text);

        message
            .blocks
            .extend(images.iter().cloned().map(Block::Image));

        message
    }

    pub fn assistant_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            blocks: vec![Block::Text { text: text.into() }],
        }
    }

    pub fn tool_results(results: Vec<ToolResult>) -> Self {
        Self {
            role: Role::User,
            blocks: results.into_iter().map(Block::ToolResult).collect(),
        }
    }

    /// All text blocks joined with blank lines.
    pub fn text(&self) -> String {
        let parts: Vec<&str> = self
            .blocks
            .iter()
            .filter_map(|block| match block {
                Block::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();

        parts.join("\n\n")
    }

    pub fn tool_calls(&self) -> Vec<ToolCall> {
        self.blocks
            .iter()
            .filter_map(|block| match block {
                Block::ToolCall(call) => Some(call.clone()),
                _ => None,
            })
            .collect()
    }
}
