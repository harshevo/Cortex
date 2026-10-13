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
            description: "Inspect the real brain state and learned intuition. `imagine` takes up to 5 next actions you are weighing, each written as `tool: target` (for example `shell: cargo test` or `edit_file: src/lib.rs`), and returns how the mind predicts each would turn out here: `p` is the chance it works, with how often the same action worked and failed in situations like this one, and what the mind would do itself. Optional neuron_ids inspects up to 8 neurons anywhere in the fly network, returning voltage and up to 64 outgoing synapses each; offset pages through their connections. Predictions and measurements are not proof of task correctness.".into(),
            input_schema: json!({"type":"object","properties":{"imagine":{"type":"array","maxItems":5,"items":{"type":"string"}},"neuron_ids":{"type":"array","maxItems":8,"items":{"type":"integer","minimum":0}},"offset":{"type":"integer","minimum":0},"sample_count":{"type":"integer","enum":[96,256,512]}},"additionalProperties":false}),
        }
    }
    fn access(&self) -> Access {
        Access::ReadOnly
    }
    async fn run(&self, query: Value, _context: &ToolContext) -> Result<String, ToolError> {
        if !query.is_object() {
            return Err(ToolError::InvalidInput("Expected an object".into()));
        }
        if query
            .get("sample_count")
            .is_some_and(|count| !matches!(count.as_u64(), Some(96 | 256 | 512)))
        {
            return Err(ToolError::InvalidInput(
                "sample_count must be 96, 256, or 512".into(),
            ));
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
        if let Some(ids) = query.get("neuron_ids") {
            let ids = ids
                .as_array()
                .ok_or_else(|| ToolError::InvalidInput("neuron_ids must be an array".into()))?;
            if ids.len() > 8 || ids.iter().any(|id| id.as_u64().is_none()) {
                return Err(ToolError::InvalidInput(
                    "Use up to 8 non-negative integer neuron IDs".into(),
                ));
            }
        }
        if query
            .get("offset")
            .is_some_and(|offset| offset.as_u64().is_none())
        {
            return Err(ToolError::InvalidInput(
                "offset must be a non-negative integer".into(),
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
