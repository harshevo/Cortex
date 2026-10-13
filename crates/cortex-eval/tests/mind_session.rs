//! The agent with the mind attached, against the real service from the
//! `brain` project. Skipped when that project or its Python is not there.

use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cortex_brain::Mind;
use cortex_core::{
    Agent, AgentConfig, AutoApprove, Event, Inbox, ToolContext, TrajectorySink, Workspace,
};
use cortex_providers::ScriptedProvider;
use cortex_providers::scripted::{final_turn, tool_turn};
use cortex_verify::ProjectVerifier;
use serde_json::{Value, json};

const CHECK: &str = "test \"$(cat fee.txt)\" = 3";

#[derive(Default)]
struct Collector(Mutex<Vec<Event>>);

impl TrajectorySink for Collector {
    fn record(&self, event: &Event) {
        self.0.lock().unwrap().push(event.clone());
    }
}

impl Collector {
    /// Everything the mind put in front of the model, briefing and advice.
    fn said_by_mind(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                Event::ContextPrepared { source, text, .. } if source == "mind" => {
                    Some(text.clone())
                }
                _ => None,
            })
            .collect()
    }

    fn tool_output(&self, tool_name: &str) -> Option<String> {
        self.0.lock().unwrap().iter().find_map(|event| match event {
            Event::ToolFinished { tool, result, .. } if tool == tool_name => {
                Some(result.output.clone())
            }
            _ => None,
        })
    }
}

struct Service(Child);

impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The real service on a free port with an empty memory, or `None` when
/// it cannot be run here.
fn start_service(memory: &std::path::Path) -> Option<(Service, String)> {
    let project = cortex_brain::mind::brain_project()?;
    let port = TcpListener::bind("127.0.0.1:0")
        .ok()?
        .local_addr()
        .ok()?
        .port();
    let python = std::env::var("CORTEX_PYTHON").unwrap_or_else(|_| "python3".to_string());

    let child = Command::new(python)
        .args([
            "-m",
            "cortex.brain.server",
            "--port",
            &port.to_string(),
            "--dir",
        ])
        .arg(memory)
        .current_dir(project)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let service = Service(child);
    let url = format!("http://127.0.0.1:{port}");
    let started = Instant::now();

    while started.elapsed() < Duration::from_secs(30) {
        if Mind::new(&url, "").health().is_some() {
            return Some((service, url));
        }

        std::thread::sleep(Duration::from_millis(200));
    }

    None
}

/// One run of the scripted model on the task, with the mind watching.
async fn run(
    root: &std::path::Path,
    mind: &Arc<Mind>,
    turns: Vec<cortex_core::ModelResponse>,
    said: Option<&str>,
) -> Arc<Collector> {
    std::fs::write(root.join("fee.txt"), "2").unwrap();

    let collector = Arc::new(Collector::default());
    let mut tools = cortex_tools::default_registry();

    tools.register(cortex_tools::BrainState(vec![mind.clone()]));

    let inbox = Arc::new(Inbox::new());

    if let Some(said) = said {
        inbox.push(said);
    }

    let agent = Agent::new(
        Arc::new(ScriptedProvider::new(turns)),
        tools,
        ToolContext {
            workspace: Workspace::new(root).unwrap(),
            approver: Arc::new(AutoApprove),
        },
        AgentConfig {
            system_prompt: "test".into(),
            max_steps: 10,
            prune: None,
            max_verification_rounds: 1,
            fold_repeats: false,
            extras: Default::default(),
        },
    )
    .with_sink(collector.clone())
    .with_sink(mind.clone())
    .with_adviser(mind.clone())
    .with_context_source(mind.clone())
    .with_inbox(inbox)
    .with_verifier(Arc::new(ProjectVerifier::with_commands(vec![CHECK.into()])));

    let mut conversation = Vec::new();

    agent.run(&mut conversation, "fix the fee").await.unwrap();

    collector
}

/// Look, see the check fail, fix, finish: the way a fix usually goes.
fn fix() -> Vec<cortex_core::ModelResponse> {
    vec![
        tool_turn(vec![("shell", json!({ "command": CHECK }))]),
        tool_turn(vec![(
            "write_file",
            json!({ "path": "fee.txt", "content": "3" }),
        )]),
        final_turn("The fee is 3 now."),
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn the_mind_learns_from_runs_and_from_the_person_and_is_measured() {
    let memory = tempfile::tempdir().unwrap();

    let Some((_service, url)) = start_service(memory.path()) else {
        eprintln!("skipped: the brain project's service could not be started here");

        return;
    };

    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let mind = Arc::new(Mind::new(&url, "fees").with_control(0.0));

    // It learns from the agent: three passing runs that each begin with
    // a check that fails.
    for _ in 0..3 {
        run(root, &mind, fix(), None).await;
    }

    // Intuition: it now says what works here, and the check that failed on
    // the way to a pass is not held against the agent. Imagination: the
    // model asks how two actions would turn out and is answered.
    let mut turns = fix();

    turns.insert(
        1,
        tool_turn(vec![(
            "brain_state",
            json!({ "imagine": ["write_file: fee.txt", "shell: rm fee.txt"] }),
        )]),
    );

    let fourth = run(root, &mind, turns, None).await;
    let said = fourth.said_by_mind().join("\n");

    assert!(said.contains("shell: test"), "the mind said: {said}");
    assert!(!said.contains("failed"), "the mind said: {said}");

    let imagined: Value =
        serde_json::from_str(&fourth.tool_output("brain_state").unwrap()).unwrap();
    let options = imagined[0]["options"].as_array().unwrap();
    let chance = |action: &str| {
        options
            .iter()
            .find(|option| option["action"] == action)
            .map(|option| option["p"].as_f64().unwrap())
            .unwrap()
    };

    assert_eq!(imagined[0]["source"], "mind");
    assert!(chance("write_file: fee.txt") > chance("shell: rm fee.txt"));

    // It learns from the person: what they say during a run is kept, and
    // said back the next time the same ground is covered.
    run(
        root,
        &mind,
        fix(),
        Some("no, never write fee.txt by hand, use the fee script"),
    )
    .await;

    let sixth = run(root, &mind, fix(), None).await;
    let said = sixth.said_by_mind().join("\n");

    assert!(
        said.contains("the person said") && said.contains("use the fee script"),
        "the mind said: {said}"
    );

    // And every run was measured.
    let health = mind.health().unwrap();
    let advised = &health["measured"]["advised"];

    assert_eq!(health["tasks"], 6);
    assert_eq!(health["told"], 1);
    assert_eq!(advised["runs"], 6);
    assert_eq!(advised["checked"], 6);
    assert_eq!(advised["passed"], 1.0);
    assert!(advised["steps"].as_f64().unwrap() >= 3.0);
    assert!(advised["advice_lines"].as_f64().unwrap() > 0.0);
}
