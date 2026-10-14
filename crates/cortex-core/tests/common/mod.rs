//! What the tests of the loop share: a project, a tool, and ways to
//! script a model and to hear what a run reports.

#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use cortex_core::{
    Access, Agent, AgentConfig, AutoApprove, Block, Event, Extras, Message, ModelProvider,
    ModelResponse, Role, Tool, ToolContext, ToolError, ToolRegistry, ToolSpec, TrajectorySink,
    Usage, Workspace,
};
use cortex_providers::scripted::{final_turn, tool_turn};
use serde_json::{Value, json};

/// A tool that answers with as many characters as it is asked for.
pub struct Fill;

#[async_trait]
impl Tool for Fill {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "fill".to_string(),
            description: "Returns text of the length asked for.".to_string(),
            input_schema: json!({ "type": "object" }),
        }
    }

    fn access(&self) -> Access {
        Access::ReadOnly
    }

    async fn run(&self, input: Value, _context: &ToolContext) -> Result<String, ToolError> {
        let chars = input["chars"].as_u64().unwrap_or(10) as usize;

        Ok("x".repeat(chars))
    }
}

/// Keeps every event of a run.
#[derive(Default)]
pub struct Heard(Mutex<Vec<Event>>);

impl Heard {
    pub fn events(&self) -> Vec<Event> {
        self.0.lock().unwrap().clone()
    }
}

impl TrajectorySink for Heard {
    fn record(&self, event: &Event) {
        self.0.lock().unwrap().push(event.clone());
    }
}

pub fn context() -> (tempfile::TempDir, ToolContext) {
    let dir = tempfile::tempdir().unwrap();

    let context = ToolContext {
        workspace: Workspace::new(dir.path()).unwrap(),
        approver: Arc::new(AutoApprove),
    };

    (dir, context)
}

/// An agent with the one tool, and the sink that hears its runs.
pub fn agent(
    provider: Arc<dyn ModelProvider>,
    context: ToolContext,
    extras: Extras,
) -> (Agent, Arc<Heard>) {
    let mut tools = ToolRegistry::new();

    tools.register(Fill);

    let config = AgentConfig {
        system_prompt: "test".to_string(),
        max_steps: 20,
        prune: None,
        max_verification_rounds: 0,
        fold_repeats: false,
        extras,
    };

    let heard = Arc::new(Heard::default());

    let agent = Agent::new(provider, tools, context, config).with_sink(heard.clone());

    (agent, heard)
}

/// The model calls the tool, and reports that this call's input was
/// `input` tokens.
pub fn calls_tool(input: u64) -> ModelResponse {
    with_input(tool_turn(vec![("fill", json!({ "chars": 400 }))]), input)
}

/// The model answers and stops.
pub fn says(text: &str, input: u64) -> ModelResponse {
    with_input(final_turn(text), input)
}

fn with_input(mut response: ModelResponse, input: u64) -> ModelResponse {
    response.usage = Usage {
        input_tokens: input,
        output_tokens: 20,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
    };

    response
}

/// Whether every tool result answers a call in the message before it,
/// every call is answered, and the conversation starts with the person.
pub fn is_well_formed(conversation: &[Message]) -> bool {
    if conversation
        .first()
        .is_some_and(|first| first.role != Role::User)
    {
        return false;
    }

    for (index, message) in conversation.iter().enumerate() {
        let results: Vec<&str> = message
            .blocks
            .iter()
            .filter_map(|block| match block {
                Block::ToolResult(result) => Some(result.call_id.as_str()),
                _ => None,
            })
            .collect();

        let asked: Vec<String> = match index.checked_sub(1).map(|before| &conversation[before]) {
            Some(before) if before.role == Role::Assistant => before
                .tool_calls()
                .into_iter()
                .map(|call| call.id)
                .collect(),

            _ => Vec::new(),
        };

        if results
            .iter()
            .any(|id| !asked.iter().any(|call| call == id))
        {
            return false;
        }

        if message.role == Role::User && asked.iter().any(|call| !results.contains(&call.as_str()))
        {
            return false;
        }
    }

    true
}
