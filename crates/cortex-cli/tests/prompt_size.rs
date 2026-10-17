//! What every request carries before the conversation: the system prompt
//! and the descriptions of the tools. Both are sent on every model call,
//! and a provider can only reuse its cache while they stay byte for byte
//! the same, so they must come out identical every time they are built.
//!
//! Run with `--nocapture` to see the sizes.

use clap::Parser;
use cortex_cli::args::ModelArgs;
use cortex_cli::features::{self, SessionLocation};
use cortex_core::window::tool_spec_chars;
use cortex_core::{ToolSpec, Workspace, prompt};

#[derive(Parser)]
struct Arguments {
    #[command(flatten)]
    model: ModelArgs,
}

/// The system prompt and the tool descriptions an agent built with
/// `flags` would send.
async fn built(project: &std::path::Path, flags: &[&str]) -> (String, Vec<ToolSpec>) {
    let mut arguments = vec!["cortex", "--no-records"];

    arguments.extend(flags);

    let model = Arguments::parse_from(arguments).model;

    let location = SessionLocation {
        directory: project.join(".cortex"),
        scope: project.display().to_string(),
        project: None,
    };

    let setup = features::setup(&model, location).await.unwrap();

    let workspace = Workspace::new(project).unwrap();

    let system = prompt::system_prompt_with(&workspace, &setup.notes(&workspace));

    (system, setup.tools.specs())
}

const EVERYTHING: [&str; 20] = [
    "--provider",
    "codex",
    "--with",
    "lean",
    "--with",
    "index",
    "--with",
    "graph",
    "--with",
    "batch",
    "--with",
    "repro",
    "--with",
    "skills",
    "--with",
    "github",
    "--with",
    "browser",
    "--with",
    "gate",
];

#[tokio::test]
async fn the_start_of_every_request_is_the_same_each_time_it_is_built() {
    let project = tempfile::tempdir().unwrap();

    let first = built(project.path(), &EVERYTHING).await;
    let second = built(project.path(), &EVERYTHING).await;

    assert_eq!(
        first.0, second.0,
        "the system prompt changed between builds"
    );
    assert_eq!(first.1, second.1, "the tools changed between builds");

    // Serialised, too: a schema whose keys came out in another order
    // would be a different request.
    let text = |specs: &[ToolSpec]| serde_json::to_string(specs).unwrap();

    assert_eq!(text(&first.1), text(&second.1));

    let names: Vec<&str> = first.1.iter().map(|tool| tool.name.as_str()).collect();
    let mut sorted = names.clone();

    sorted.sort_unstable();

    assert_eq!(names, sorted, "tools are sent in name order");
}

#[tokio::test]
async fn sizes_of_what_every_request_carries() {
    let project = tempfile::tempdir().unwrap();

    for (label, flags) in [
        ("plain", &[][..]),
        ("lean", &["--with", "lean"][..]),
        ("everything", &EVERYTHING[..]),
    ] {
        let (system, tools) = built(project.path(), flags).await;

        println!(
            "{label}: system prompt {} chars, {} tools {} chars",
            system.len(),
            tools.len(),
            tool_spec_chars(&tools)
        );

        for tool in &tools {
            println!(
                "    {:<22} {:>6}",
                tool.name,
                tool_spec_chars(std::slice::from_ref(tool))
            );
        }

        // A prompt that grows past this deserves a look before it ships.
        assert!(system.len() < 40_000, "{label}: {} chars", system.len());
    }
}
