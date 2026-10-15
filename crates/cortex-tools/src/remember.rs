use std::sync::Arc;

use async_trait::async_trait;
use cortex_brain::memory::MemorySystem;
use cortex_core::{Access, Tool, ToolContext, ToolError, ToolSpec};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::input;

const MAX_FACT_CHARS: usize = 300;

/// Saves a fact about the repository for later tasks.
pub struct Remember {
    memory: Arc<MemorySystem>,
}

impl Remember {
    pub fn new(memory: Arc<MemorySystem>) -> Self {
        Self { memory }
    }
}

#[derive(Deserialize)]
struct Input {
    fact: String,

    #[serde(default)]
    paths: Vec<String>,
}

#[async_trait]
impl Tool for Remember {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "remember".to_string(),

            description: "Save one fact about this repository so that later tasks start with \
                          it. Use it for things that took effort to find out and will stay \
                          true: where a kind of logic lives, a convention the code follows, a \
                          command that must be run a particular way. Do not save details of \
                          the current task; what you changed and ran is recorded automatically. \
                          Call it alongside other tools rather than on its own."
                .to_string(),

            input_schema: json!({
                "type": "object",
                "properties": {
                    "fact": {
                        "type": "string",
                        "description": "One sentence, understandable without this conversation.",
                    },
                    "paths": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Files the fact is about. The fact is dropped if one is deleted.",
                    },
                },
                "required": ["fact"],
            }),
        }
    }

    // It writes only to Cortex's own memory, never to the project.
    fn access(&self) -> Access {
        Access::ReadOnly
    }

    async fn run(&self, input: Value, context: &ToolContext) -> Result<String, ToolError> {
        let input: Input = input::parse(input)?;

        let fact = input.fact.trim();

        if fact.is_empty() {
            return Err(ToolError::InvalidInput("`fact` is empty".to_string()));
        }

        if fact.chars().count() > MAX_FACT_CHARS {
            return Err(ToolError::InvalidInput(format!(
                "keep the fact under {MAX_FACT_CHARS} characters"
            )));
        }

        for path in &input.paths {
            if !context.workspace.resolve(path)?.exists() {
                return Err(ToolError::InvalidInput(format!(
                    "`{path}` does not exist; list only real files in `paths`"
                )));
            }
        }

        self.memory
            .remember(fact, input.paths)
            .map_err(|error| ToolError::Failed(error.to_string()))?;

        Ok("Saved.".to_string())
    }
}
