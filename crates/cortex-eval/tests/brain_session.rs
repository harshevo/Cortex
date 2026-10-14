use cortex_brain::{Controller, DirectEncoder, Intuition};
use cortex_living::{Counsel, Living};
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
    let brain_file = root.join("memory.controller.json");
    let network_file = root.join("network.json");
    let intuition_file = root.join("memory.intuition.json");
    let log_file = root.join("chat.jsonl");
    let memory =
        Arc::new(MemorySystem::open(&memory_file, MemoryConfig::for_scope(&scope)).unwrap());
    let brain = Arc::new(Controller::new(
        Box::new(DirectEncoder),
        Some(brain_file.clone()),
    ));
    let living = Arc::new(Living::open(Some(network_file.clone())));
    let counsel = Arc::new(Counsel(living.clone()));
    let intuition = Arc::new(Intuition::new(Some(intuition_file.clone())));
    let mut tools = cortex_tools::default_registry();
    cortex_tools::add_memory_tools(&mut tools, memory.clone());
    tools.register(cortex_tools::BrainState(vec![
        brain.clone(),
        counsel.clone(),
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
    .with_sink(living.clone())
    .with_adviser(counsel.clone())
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
    // What the dendritic network made of the run is logged, and none of
    // it is put in front of the model.
    let observed: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            Event::BrainObserved { source, detail, .. } if source == "dendritic network" => {
                Some(detail)
            }
            _ => None,
        })
        .collect();
    assert!(!observed.is_empty());
    assert!(observed.last().unwrap()["neurons"].as_u64().unwrap() > 6);
    assert!(
        conversation
            .iter()
            .all(|message| !message.text().contains("dendritic network"))
    );
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
    assert_eq!(
        Controller::new(Box::new(DirectEncoder), Some(brain_file)).runs(),
        1
    );
    assert_eq!(Intuition::new(Some(intuition_file)).runs(), 1);

    // The network grew from the run, kept it, and answers the model's
    // own questions about what it has seen.
    let lived = living.snapshot();
    assert!(lived.meters.moments >= 4);
    assert!(lived.meters.branched > 0);
    assert_eq!(
        Living::open(Some(network_file)).snapshot().neurons.len(),
        lived.neurons.len()
    );
    let imagined = cortex_core::Adviser::inspect(
        counsel.as_ref(),
        &json!({"imagine": ["write_file: capacity.txt"]}),
    )
    .unwrap();
    assert_eq!(imagined["imagined"][0]["met_before"], true);
}
