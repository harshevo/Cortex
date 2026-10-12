//! The interface every model backend implements.

use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::ProviderError;
use crate::message::Message;
use crate::usage::Usage;

/// A tool as advertised to the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

pub struct ModelRequest<'a> {
    pub system: &'a str,
    pub messages: &'a [Message],
    pub tools: &'a [ToolSpec],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// The model finished its turn.
    EndTurn,

    /// The model wants tools run before it continues.
    ToolUse,

    /// The response hit the output limit and is incomplete.
    MaxTokens,

    /// The provider declined the request.
    Refusal,

    /// The provider paused a long turn; send the conversation back to resume.
    Paused,

    Other(String),
}

#[derive(Debug, Clone)]
pub struct ModelResponse {
    pub message: Message,
    pub stop_reason: StopReason,
    pub usage: Usage,
}

/// Watches a response while the model is still writing it.
///
/// What an observer sees is for showing progress only. The finished
/// response is the one the provider returns.
pub trait StreamObserver: Send + Sync {
    /// A request has gone out. Called again if it has to be retried, in
    /// which case anything seen so far belongs to the abandoned attempt.
    fn started(&self) {}

    /// The next piece of the model's text.
    fn text(&self, piece: &str);

    /// The next piece of the model's reasoning, from a provider that
    /// shows it. It is for watching only and is not part of the response.
    fn thinking(&self, _piece: &str) {}

    /// The model has begun asking for the tool of this name. Its
    /// arguments are still being written.
    fn calling(&self, _tool: &str) {}

    /// The request failed in a way that may not last, and goes out again
    /// after `wait`.
    fn retrying(&self, _error: &ProviderError, _wait: Duration) {}
}

#[async_trait]
pub trait ModelProvider: Send + Sync {
    /// Short provider name, for example `anthropic`.
    fn name(&self) -> &str;

    /// The model this provider instance talks to.
    fn model(&self) -> &str;

    async fn complete(&self, request: ModelRequest<'_>) -> Result<ModelResponse, ProviderError>;

    /// Like [`ModelProvider::complete`], but tells `observer` about the
    /// text as it arrives. A provider that cannot stream answers all at
    /// once, and the observer hears nothing until then.
    async fn complete_streaming(
        &self,
        request: ModelRequest<'_>,
        observer: &dyn StreamObserver,
    ) -> Result<ModelResponse, ProviderError> {
        let _ = observer;

        self.complete(request).await
    }
}
