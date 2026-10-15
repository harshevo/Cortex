use async_trait::async_trait;
use cortex_core::{Access, Adviser, Tool, ToolContext, ToolError, ToolSpec};
use serde_json::{Value, json};
use std::sync::Arc;

pub struct BrainState(pub Vec<Arc<dyn Adviser>>);

#[async_trait]
impl Tool for BrainState {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "brain_state".into(),
            description: "Ask the brain what it expects from here. `imagine` takes up to 5 next actions you are weighing, each written as `tool: target` (for example `shell: cargo test` or `edit_file: src/lib.rs`): for each it follows the way on it has seen from there, says how sure it is of each step, and whether that way has ended in good or bad news (checks passing, the person agreeing). `goal` takes an outcome such as `checks: passed` and returns a way from here to it through what it expects. With neither, it returns what it expects next and what it holds in mind. These are patterns learned from experience, not proof of correctness.".into(),
            input_schema: json!({"type":"object","properties":{"imagine":{"type":"array","maxItems":5,"items":{"type":"string"}},"goal":{"type":"string","maxLength":256}},"additionalProperties":false}),
        }
    }
    fn access(&self) -> Access {
        Access::ReadOnly
    }
    async fn run(&self, query: Value, _context: &ToolContext) -> Result<String, ToolError> {
        if !query.is_object() {
            return Err(ToolError::InvalidInput("Expected an object".into()));
        }
        if let Some(actions) = query.get("imagine") {
            let actions = actions
                .as_array()
                .ok_or_else(|| ToolError::InvalidInput("imagine must be an array".into()))?;
            if actions.len() > 5 || actions.iter().any(|action| !action.is_string()) {
                return Err(ToolError::InvalidInput(
                    "Give up to 5 actions as strings like `shell: cargo test`".into(),
                ));
            }
        }
        if query.get("goal").is_some_and(|goal| !goal.is_string()) {
            return Err(ToolError::InvalidInput(
                "goal must be a string such as `checks: passed`".into(),
            ));
        }
        let states: Vec<_> = self
            .0
            .iter()
            .filter_map(|adviser| adviser.inspect(&query))
            .collect();
        serde_json::to_string(&states).map_err(|error| ToolError::Failed(error.to_string()))
    }
}
