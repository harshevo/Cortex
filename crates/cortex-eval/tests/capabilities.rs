//! End-to-end checks of the verification gate, with a scripted
//! model so they run without a network connection or an API key.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cortex_eval::{RunOptions, Suite, SuiteReport, run_suite};
use cortex_providers::ScriptedProvider;
use cortex_providers::scripted::{final_turn, tool_turn};
use cortex_verify::ProjectVerifier;
use serde_json::{Value, json};

/// A check the project could plausibly have: the first page starts at the
/// first item. It fails on the untouched task and passes once it is fixed.
const FIRST_PAGE_CHECK: &str =
    "python3 -c \"import pagination; assert pagination.paginate([1, 2, 3], 1, 2) == [1, 2]\"";

fn suite() -> Suite {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../eval/suites/smoke");

    Suite::load(&dir).unwrap().filtered(Some("fix-pagination"))
}

fn fix_first_page() -> cortex_core::ModelResponse {
    tool_turn(vec![(
        "edit_file",
        json!({
            "path": "pagination.py",
            "old_text": "start = page * page_size",
            "new_text": "start = (page - 1) * page_size",
        }),
    )])
}

fn fix_page_count() -> cortex_core::ModelResponse {
    tool_turn(vec![(
        "edit_file",
        json!({
            "path": "pagination.py",
            "old_text": "return len(items) // page_size",
            "new_text": "return (len(items) + page_size - 1) // page_size",
        }),
    )])
}

/// The events of one attempt's trajectory file.
fn events(run_dir: &Path, attempt: u32) -> Vec<Value> {
    let file = run_dir.join(format!("trajectories/fix-pagination-{attempt}.jsonl"));

    fs::read_to_string(file)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn of_type<'a>(events: &'a [Value], kind: &str) -> Vec<&'a Value> {
    events
        .iter()
        .filter(|event| event["type"] == kind)
        .collect()
}

async fn run(options: RunOptions, provider: ScriptedProvider) -> (SuiteReport, PathBuf) {
    run_suite(&suite(), Arc::new(provider), &options, |_| {})
        .await
        .unwrap()
}

#[tokio::test]
async fn the_gate_sends_unfinished_work_back() {
    let results = tempfile::tempdir().unwrap();

    let options = RunOptions {
        verifier: Some(Arc::new(ProjectVerifier::with_commands(vec![
            FIRST_PAGE_CHECK.to_string(),
        ]))),
        ..RunOptions::plain("gate", results.path(), cortex_tools::default_registry())
    };

    // The model fixes one of the two problems and claims to be finished.
    // Only after the gate objects does it fix the one the check covers.
    let provider = ScriptedProvider::new(vec![
        fix_page_count(),
        final_turn("Fixed."),
        fix_first_page(),
        final_turn("Fixed both problems."),
    ]);

    let (report, run_dir) = run(options, provider).await;

    let result = &report.results[0];

    assert!(result.solved);
    assert_eq!(result.verified, Some(true));
    assert_eq!(result.steps, 4);

    let events = events(&run_dir, 1);
    let verifications = of_type(&events, "verification_ran");

    assert_eq!(verifications.len(), 2);
    assert_eq!(verifications[0]["passed"], false);
    assert_eq!(verifications[1]["passed"], true);

    // The failure reached the model as text.
    assert!(
        verifications[0]["checks"][0]["output"]
            .as_str()
            .unwrap()
            .contains("AssertionError")
    );
}

#[tokio::test]
async fn the_gate_gives_up_after_its_rounds_and_reports_unverified() {
    let results = tempfile::tempdir().unwrap();

    let options = RunOptions {
        verifier: Some(Arc::new(ProjectVerifier::with_commands(vec![
            FIRST_PAGE_CHECK.to_string(),
        ]))),
        max_verification_rounds: 1,
        ..RunOptions::plain("stubborn", results.path(), cortex_tools::default_registry())
    };

    let provider = ScriptedProvider::new(vec![
        fix_page_count(),
        final_turn("Fixed."),
        final_turn("It is fixed, I am sure."),
    ]);

    let (report, _run_dir) = run(options, provider).await;

    let result = &report.results[0];

    assert!(!result.solved);
    assert_eq!(result.verified, Some(false));
    assert_eq!(result.steps, 3);
}

#[tokio::test]
async fn a_run_that_changes_nothing_is_not_checked() {
    let results = tempfile::tempdir().unwrap();

    let options = RunOptions {
        verifier: Some(Arc::new(ProjectVerifier::with_commands(vec![
            FIRST_PAGE_CHECK.to_string(),
        ]))),
        ..RunOptions::plain("idle", results.path(), cortex_tools::default_registry())
    };

    let provider = ScriptedProvider::new(vec![
        tool_turn(vec![("read_file", json!({ "path": "pagination.py" }))]),
        final_turn("The first page starts one page too late."),
    ]);

    let (report, _run_dir) = run(options, provider).await;

    assert_eq!(report.results[0].verified, None);
    assert_eq!(report.results[0].steps, 2);
}
