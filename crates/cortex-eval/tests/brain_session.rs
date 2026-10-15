use cortex_brain::memory::{MemoryConfig, MemorySystem};
use cortex_brain::{Counsel, Link};
use cortex_core::{
    Agent, AgentConfig, AutoApprove, Event, JsonlSink, ToolContext, Workspace, replay,
};
use cortex_providers::ScriptedProvider;
use cortex_providers::scripted::{final_turn, tool_turn};
use cortex_verify::ProjectVerifier;
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn session_connects_tools_the_brain_verification_memory_and_replay() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let scope = root.display().to_string();
    let memory_file = root.join("memory.db");
    let brain_file = root.join("brain.bin");
    let log_file = root.join("chat.jsonl");
    let memory =
        Arc::new(MemorySystem::open(&memory_file, MemoryConfig::for_scope(&scope)).unwrap());
    let brain = Arc::new(Link::open(Some(brain_file.clone())));
    let sink = brain.sink();
    let counsel = Arc::new(Counsel::of(&sink));
    let mut tools = cortex_tools::default_registry();
    cortex_tools::add_memory_tools(&mut tools, memory.clone());
    tools.register(cortex_tools::BrainState(vec![counsel.clone()]));
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
    .with_sink(sink.clone())
    .with_adviser(counsel.clone())
    .with_verifier(Arc::new(ProjectVerifier::with_commands(vec![
        "test \"$(cat capacity.txt)\" = 168".into(),
    ])));
    let mut conversation = Vec::new();
    let report = agent
        .run(&mut conversation, "calculate calendar capacity and save it")
        .await
        .unwrap();
    assert_eq!(report.verified, Some(true));
    let events: Vec<Event> = std::fs::read_to_string(&log_file)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    // What the brain made of the run is logged, and none of it is put in
    // front of the model unless it advises.
    let observed: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            Event::BrainObserved { source, detail, .. } if source == "brain" => Some(detail),
            _ => None,
        })
        .collect();
    assert!(!observed.is_empty());
    assert!(observed.last().unwrap()["moments"].as_u64().unwrap() >= 4);
    let replayed = replay::conversation(&events);
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

    // The brain lived the run as a stream, kept it, and answers the
    // model's own questions about what it has seen.
    let lived = brain.snapshot();
    assert!(lived.mind.senses.moments >= 4);
    assert!(lived.mind.streams.contains(&sink.stream().to_string()));
    brain.checkpoint().unwrap();
    let reloaded = Link::open(Some(brain_file)).snapshot();
    assert_eq!(reloaded.mind.senses.moments, lived.mind.senses.moments);
    assert_eq!(reloaded.cells, lived.cells);
    let imagined = cortex_core::Adviser::inspect(
        counsel.as_ref(),
        &json!({"imagine": ["write_file: capacity.txt"]}),
    )
    .unwrap();
    assert_eq!(imagined["imagined"][0]["met_before"], true);
}
