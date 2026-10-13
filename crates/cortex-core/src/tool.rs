//! The interface every tool implements, and the registry that runs them.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::approval::{ApprovalRequest, Approver};
use crate::error::ToolError;
use crate::message::{Image, ToolCall, ToolResult};
use crate::provider::ToolSpec;
use crate::workspace::Workspace;

/// Tool output longer than this is cut in the middle before the model sees it.
pub const MAX_OUTPUT_CHARS: usize = 30_000;

/// Whether a tool only looks, or can change files and run commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    ReadOnly,
    Mutating,
}

/// Everything a tool needs from its surroundings.
#[derive(Clone)]
pub struct ToolContext {
    pub workspace: Workspace,
    pub approver: Arc<dyn Approver>,
}

/// What a tool hands back: text, and sometimes pictures with it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolOutput {
    pub text: String,

    /// For the model to look at, such as a screenshot.
    pub images: Vec<Image>,
}

impl ToolOutput {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            images: Vec::new(),
        }
    }
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;

    fn access(&self) -> Access;

    /// One line describing a specific call, shown when asking for approval.
    fn describe(&self, input: &Value) -> String {
        input.to_string()
    }

    async fn run(&self, input: Value, context: &ToolContext) -> Result<String, ToolError>;

    /// The same call for a tool whose answer can carry pictures. This is
    /// what the registry runs; a tool that only returns text leaves it be.
    async fn run_with_images(
        &self,
        input: Value,
        context: &ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        self.run(input, context).await.map(ToolOutput::text)
    }
}

/// The set of tools available to an agent.
///
/// Tools are kept sorted by name so the list sent to the model is identical
/// on every request, which keeps the provider's prompt cache valid.
#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, tool: impl Tool + 'static) {
        let name = tool.spec().name;

        self.tools.insert(name, Arc::new(tool));
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.values().map(|tool| tool.spec()).collect()
    }

    /// Whether `name` is a tool that can change files or run commands.
    pub fn is_mutating(&self, name: &str) -> bool {
        self.tools
            .get(name)
            .is_some_and(|tool| tool.access() == Access::Mutating)
    }

    /// Runs one call. Failures become error results rather than Rust errors,
    /// because the model is the one that has to react to them.
    pub async fn dispatch(&self, call: &ToolCall, context: &ToolContext) -> ToolResult {
        let outcome = self.try_dispatch(call, context).await;

        let (output, is_error) = match outcome {
            Ok(output) => (output, false),
            Err(error) => (ToolOutput::text(error.to_string()), true),
        };

        ToolResult {
            call_id: call.id.clone(),
            output: truncate_middle(&output.text, MAX_OUTPUT_CHARS),
            is_error,
            images: output.images,
        }
    }

    async fn try_dispatch(
        &self,
        call: &ToolCall,
        context: &ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        let tool = self.tools.get(&call.name).ok_or_else(|| {
            ToolError::InvalidInput(format!("there is no tool named `{}`", call.name))
        })?;

        if tool.access() == Access::Mutating {
            let request = ApprovalRequest {
                tool: call.name.clone(),
                summary: tool.describe(&call.input),
            };

            if !context.approver.approve(&request).await {
                return Err(ToolError::Denied);
            }
        }

        tool.run_with_images(call.input.clone(), context).await
    }
}

/// Keeps the start and end of long output, where the useful parts usually
/// are, and says how much was removed.
pub fn truncate_middle(text: &str, max_chars: usize) -> String {
    let total = text.chars().count();

    if total <= max_chars {
        return text.to_string();
    }

    let head_len = max_chars * 2 / 3;
    let tail_len = max_chars - head_len;

    let head: String = text.chars().take(head_len).collect();
    let tail: String = text.chars().skip(total - tail_len).collect();

    let removed = total - head_len - tail_len;

    format!("{head}\n\n[... {removed} characters omitted ...]\n\n{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_output_is_untouched() {
        assert_eq!(truncate_middle("hello", 10), "hello");
    }

    #[test]
    fn long_output_keeps_both_ends() {
        let text = format!("{}{}", "a".repeat(50), "z".repeat(50));

        let truncated = truncate_middle(&text, 30);

        assert!(truncated.starts_with("aaaa"));
        assert!(truncated.ends_with("zzzz"));
        assert!(truncated.contains("70 characters omitted"));
    }
}
