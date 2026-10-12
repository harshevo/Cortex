//! End-to-end checks of what keeps a session going: telling a run
//! something while it works, and taking a session up again from its log.
//! A scripted model stands in for a real one.

use std::fs;
use std::sync::Arc;

use cortex_core::{
    Agent, AgentConfig, AutoApprove, Block, Event, Inbox, JsonlSink, Message, Role, ToolContext,
    Workspace, replay,
};
use cortex_providers::ScriptedProvider;
use cortex_providers::scripted::{final_turn, tool_turn};
use serde_json::json;

fn project() -> (tempfile::TempDir, ToolContext) {
    let dir = tempfile::tempdir().unwrap();

    fs::write(dir.path().join("notes.txt"), "keep the ledger balanced\n").unwrap();

    let context = ToolContext {
        workspace: Workspace::new(dir.path()).unwrap(),
        approver: Arc::new(AutoApprove),
    };

    (dir, context)
}

fn config() -> AgentConfig {
    AgentConfig {
        system_prompt: "test".to_string(),
        max_steps: 20,
        prune: None,
        max_verification_rounds: 0,
        fold_repeats: false,
        extras: Default::default(),
    }
}

fn agent(script: ScriptedProvider, context: ToolContext) -> Agent {
    Agent::new(
        Arc::new(script),
        cortex_tools::default_registry(),
        context,
        config(),
    )
}

fn read_notes() -> cortex_core::ModelResponse {
    tool_turn(vec![("read_file", json!({ "path": "notes.txt" }))])
}

#[tokio::test]
async fn what_is_said_mid_run_reaches_the_model_with_the_next_results() {
    let (_dir, context) = project();

    let inbox = Arc::new(Inbox::new());

    let agent = agent(
        ScriptedProvider::new(vec![read_notes(), final_turn("Done.")]),
        context,
    )
    .with_inbox(inbox.clone());

    inbox.push("leave the tests alone");

    let mut conversation = Vec::new();

    let report = agent.run(&mut conversation, "tidy up").await.unwrap();

    assert_eq!(report.steps, 2);

    // Task, the model's call, then the result with the message after it.
    let answer = &conversation[2];

    assert_eq!(answer.role, Role::User);
    assert!(matches!(answer.blocks[0], Block::ToolResult(_)));
    assert!(answer.text().ends_with("leave the tests alone"));

    assert!(inbox.take().is_empty());
}

#[tokio::test]
async fn what_is_said_as_the_model_finishes_keeps_the_run_going() {
    let (_dir, context) = project();

    let inbox = Arc::new(Inbox::new());

    let agent = agent(
        ScriptedProvider::new(vec![
            final_turn("Done."),
            final_turn("Done, with that too."),
        ]),
        context,
    )
    .with_inbox(inbox.clone());

    inbox.push("also update the notes");

    let mut conversation = Vec::new();

    let report = agent.run(&mut conversation, "tidy up").await.unwrap();

    assert_eq!(report.steps, 2);
    assert_eq!(report.final_text, "Done, with that too.");

    assert_eq!(conversation[2].role, Role::User);
    assert!(conversation[2].text().ends_with("also update the notes"));
}

#[tokio::test]
async fn a_session_read_back_from_its_log_is_the_conversation_it_had() {
    let (dir, context) = project();

    let log = dir.path().join("session.jsonl");

    let inbox = Arc::new(Inbox::new());

    let script = ScriptedProvider::new(vec![
        read_notes(),
        final_turn("Read it."),
        read_notes(),
        final_turn("Read it again."),
    ]);

    let agent = agent(script, context)
        .with_sink(Arc::new(JsonlSink::create(&log).unwrap()))
        .with_inbox(inbox.clone());

    let mut conversation = Vec::new();

    agent
        .run(&mut conversation, "read the notes")
        .await
        .unwrap();

    inbox.push("more slowly this time");

    agent.run(&mut conversation, "and again").await.unwrap();

    let events: Vec<Event> = fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();

    let read_back: Vec<Message> = replay::conversation(&events);

    assert_eq!(read_back, conversation);
    assert_eq!(read_back.len(), 8);
}

#[tokio::test]
async fn a_session_taken_up_again_is_added_to_its_own_log() {
    let (dir, context) = project();

    let log = dir.path().join("session.jsonl");

    let first = agent(
        ScriptedProvider::new(vec![final_turn("One.")]),
        context.clone(),
    )
    .with_sink(Arc::new(JsonlSink::create(&log).unwrap()));

    first.run(&mut Vec::new(), "first").await.unwrap();

    let events = |log: &std::path::Path| -> Vec<Event> {
        fs::read_to_string(log)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    };

    // Later, perhaps after the app was closed: the same session again.
    let mut conversation = replay::conversation(&events(&log));

    let second = agent(ScriptedProvider::new(vec![final_turn("Two.")]), context)
        .with_sink(Arc::new(JsonlSink::append(&log).unwrap()));

    second.run(&mut conversation, "second").await.unwrap();

    let texts: Vec<String> = conversation.iter().map(Message::text).collect();

    assert_eq!(texts, ["first", "One.", "second", "Two."]);
    assert_eq!(replay::conversation(&events(&log)), conversation);
}
