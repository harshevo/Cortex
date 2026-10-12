//! Model backends.
//!
//! Each backend implements [`cortex_core::ModelProvider`]. Adding a provider
//! means adding a module here; nothing in the agent loop changes.

pub mod anthropic;
pub mod codex;
pub mod openai_compat;
pub mod pricing;
pub mod scripted;

mod retry;

pub use anthropic::{AnthropicConfig, AnthropicProvider};
pub use codex::{CodexConfig, CodexProvider};
pub use openai_compat::{OpenAiCompatConfig, OpenAiCompatProvider};
pub use scripted::ScriptedProvider;
