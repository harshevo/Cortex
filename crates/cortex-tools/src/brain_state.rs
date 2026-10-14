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
            description: "Ask the learning networks what they expect. `imagine` takes up to 5 next actions you are weighing, each written as `tool: target` (for example `shell: cargo test` or `edit_file: src/lib.rs`). Each network that is on answers in its own words: the mind gives `p`, the chance the action works here, with how often it worked and failed in situations like this one; the dendritic network gives the steps it has seen follow that action from here, how sure it is of each, and whether they ended in good or bad news. `goal` takes an outcome such as `checks: passed` and returns the steps that have led to it before. `language` inspects a persistent spiking dendritic learner: use `word` for observed word contexts, `complete` for an experimental continuation, or `subject` and `category` for a conditional category chain with its premises. With none, it returns what each network expects next and why. These are patterns learned from experience, not proof of correctness or fluent language ability.".into(),
            input_schema: json!({"type":"object","properties":{"imagine":{"type":"array","maxItems":5,"items":{"type":"string"}},"goal":{"type":"string"},"language":{"type":"object","description":"Inspect the persistent spiking dendritic language learner: word for learned contexts, complete for an experimental continuation, subject and category for a conditional category chain.","properties":{"word":{"type":"string","maxLength":100},"complete":{"type":"string","maxLength":4096},"tokens":{"type":"integer","minimum":1,"maximum":48},"subject":{"type":"string","maxLength":100},"category":{"type":"string","maxLength":100}},"additionalProperties":false}},"additionalProperties":false}),
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
        if let Some(language) = query.get("language") {
            if !language.is_object()
                || ["word", "complete", "subject", "category"]
                    .iter()
                    .any(|key| {
                        language.get(key).is_some_and(|v| {
                            !v.is_string() || v.as_str().unwrap_or_default().len() > 4096
                        })
                    })
                || language
                    .get("tokens")
                    .is_some_and(|v| v.as_u64().is_none_or(|n| n == 0 || n > 48))
            {
                return Err(ToolError::InvalidInput(
                    "language must contain bounded text fields and 1–48 tokens".into(),
                ));
            }
        }
        let states: Vec<_> = self
            .0
            .iter()
            .filter_map(|adviser| adviser.inspect(&query))
            .collect();
        serde_json::to_string(&states).map_err(|error| ToolError::Failed(error.to_string()))
    }
}
