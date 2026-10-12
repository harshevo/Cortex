use std::sync::Arc;

use async_trait::async_trait;
use cortex_core::{Access, Tool, ToolContext, ToolError, ToolSpec};
use cortex_memory::MemorySystem;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::input;

/// Looks things up in memory during a task.
pub struct Recall {
    memory: Arc<MemorySystem>,
}

impl Recall {
    pub fn new(memory: Arc<MemorySystem>) -> Self {
        Self { memory }
    }
}

#[derive(Deserialize)]
struct Input {
    #[serde(default)]
    query: Option<String>,
}

#[async_trait]
impl Tool for Recall {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "recall".to_string(),

            description: "Look in memory. With a `query`, returns earlier tasks in this \
                          repository that resemble it (what was changed, which commands \
                          worked, what failed) and the facts known about the repository. \
                          Without a query, returns a summary of the current task so far: files \
                          read and changed, commands that failed. The memory relevant to the \
                          task is already shown at the start, so use this only for a different \
                          question that comes up while working."
                .to_string(),

            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "What you want to know about earlier work.",
                    },
                },
            }),
        }
    }

    fn access(&self) -> Access {
        Access::ReadOnly
    }

    async fn run(&self, input: Value, context: &ToolContext) -> Result<String, ToolError> {
        let input: Input = input::parse(input)?;

        let query = input.query.as_deref().map(str::trim).unwrap_or_default();

        if query.is_empty() {
            return Ok(self.memory.working_digest());
        }

        let root = context.workspace.root();

        let recalled = self
            .memory
            .recall(query, root)
            .await
            .map_err(|error| ToolError::Failed(error.to_string()))?;

        if recalled.is_empty() {
            return Ok("Nothing in memory matches. This may be the first task here.".to_string());
        }

        Ok(recalled.render(|path| root.join(path).exists()))
    }
}
