use cortex_brain::{Brain, BrainConfig, Controller, FlyBrainEncoder, Intuition};
use cortex_core::{
    Agent, AgentConfig, AutoApprove, Event, JsonlSink, ToolContext, Workspace, replay,
};
use cortex_memory::{MemoryConfig, MemorySystem};
use cortex_providers::ScriptedProvider;
use cortex_providers::scripted::{final_turn, tool_turn};
use cortex_verify::ProjectVerifier;
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn session_connects_tools_brains_verification_memory_and_replay() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let scope = root.display().to_string();
    let memory_file = root.join("memory.db");
    let brain_file = root.join("memory.brain.json");
    let intuition_file = root.join("memory.intuition.json");
    let log_file = root.join("chat.jsonl");
    let memory =
        Arc::new(MemorySystem::open(&memory_file, MemoryConfig::for_scope(&scope)).unwrap());
    let brain = Arc::new(Controller::new(
        Box::new(FlyBrainEncoder::new(Brain::generate(BrainConfig::tiny()))),
        Some(brain_file.clone()),
    ));
    let intuition = Arc::new(Intuition::new(Some(intuition_file.clone())));
    let mut tools = cortex_tools::default_registry();
    cortex_tools::add_memory_tools(&mut tools, memory.clone());
    tools.register(cortex_tools::BrainState(vec![
        brain.clone(),
        intuition.clone(),
    ]));
    let context = ToolContext {
        workspace: Workspace::new(root).unwrap(),
        approver: Arc::new(AutoApprove),
    };
    let provider = ScriptedProvider::new(vec![
        tool_turn(vec![
            ("calculate", json!({"expression":"7*24"})),
            ("write_file", json!({"path":"capacity.txt","content":"168"})),
        ]),
        final_turn("Calendar capacity is 168 hours; saved capacity.txt."),
    ]);
    let agent = Agent::new(
        Arc::new(provider),
        tools,
        context.clone(),
        AgentConfig {
            system_prompt: "test".into(),
            max_steps: 10,
            prune: None,
            max_verification_rounds: 1,
            fold_repeats: false,
            extras: Default::default(),
        },
    )
    .with_sink(Arc::new(JsonlSink::create(&log_file).unwrap()))
    .with_sink(memory.clone())
    .with_context_source(memory.clone())
    .with_sink(brain.clone())
    .with_adviser(brain.clone())
    .with_sink(intuition.clone())
    .with_adviser(intuition.clone())
    .with_context_source(intuition.clone())
    .with_verifier(Arc::new(ProjectVerifier::with_commands(vec![
        "test \"$(cat capacity.txt)\" = 168".into(),
    ])));
    let mut conversation = Vec::new();
    let report = agent
        .run(&mut conversation, "calculate calendar capacity and save it")
        .await
        .unwrap();
    assert_eq!(report.verified, Some(true));
    assert_eq!(brain.runs(), 1);
    assert_eq!(intuition.runs(), 1);
    let events: Vec<Event> = std::fs::read_to_string(&log_file)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let traces: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            Event::BrainObserved { source, detail, .. } if source == "fly-brain" => Some(detail),
            _ => None,
        })
        .collect();
    assert!(traces.len() >= 3);
    assert_eq!(traces.last().unwrap()["runs"], 1);
    assert!(
        traces
            .iter()
            .all(|detail| detail["trace"]["frames"].as_array().unwrap().len() == 24)
    );
    // Telemetry is logged without consuming the language model's context.
    assert!(
        conversation
            .iter()
            .all(|message| !message.text().contains("neuron_ids"))
    );
    let replayed = replay::conversation(&events);
    assert!(
        replayed
            .iter()
            .all(|message| !message.text().contains("neuron_ids"))
    );
    assert!(
        replayed
            .iter()
            .any(|message| message.text().contains("168"))
    );
    drop(agent);
    drop(memory);
    let restored = MemorySystem::open(&memory_file, MemoryConfig::for_scope(&scope)).unwrap();
    let recalled = restored.recall("calendar capacity", root).await.unwrap();
    assert!(
        recalled
            .render(|path| root.join(path).exists())
            .contains("7*24 = 168")
    );
    assert_eq!(
        Controller::new(
            Box::new(FlyBrainEncoder::new(Brain::generate(BrainConfig::tiny()))),
            Some(brain_file)
        )
        .runs(),
        1
    );
    assert_eq!(Intuition::new(Some(intuition_file)).runs(), 1);
    let state =
        cortex_core::Adviser::inspect(brain.as_ref(), &json!({"neuron_ids":[0, 2999]})).unwrap();
    assert_eq!(state["neurons"].as_array().unwrap().len(), 2);
    assert_eq!(state["checked_runs"], 1);
}
