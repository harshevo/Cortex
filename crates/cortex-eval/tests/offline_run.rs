//! End-to-end check of the loop, the tools, and the harness with a scripted
//! model, so it runs without a network connection or an API key.

use std::path::PathBuf;
use std::sync::Arc;

use cortex_eval::{Comparison, RunOptions, Suite, SuiteReport, Verdict, run_suite};
use cortex_providers::ScriptedProvider;
use cortex_providers::scripted::{final_turn, tool_turn};
use serde_json::json;

fn smoke_suite(filter: &str) -> Suite {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../eval/suites/smoke");

    Suite::load(&dir).unwrap().filtered(Some(filter))
}

async fn run(label: &str, provider: ScriptedProvider) -> SuiteReport {
    let results = tempfile::tempdir().unwrap();

    let options = RunOptions {
        max_steps: 10,
        ..RunOptions::plain(label, results.path(), cortex_tools::default_registry())
    };

    let suite = smoke_suite("fix-pagination");

    let (report, run_dir) = run_suite(&suite, Arc::new(provider), &options, |_| {})
        .await
        .unwrap();

    assert!(run_dir.join("report.json").is_file());
    assert!(run_dir.join("summary.md").is_file());
    assert!(
        run_dir
            .join("trajectories/fix-pagination-1.jsonl")
            .is_file()
    );

    report
}

/// A script that reads the file, fixes both bugs, and stops.
fn fixing_script() -> ScriptedProvider {
    ScriptedProvider::new(vec![
        tool_turn(vec![("read_file", json!({ "path": "pagination.py" }))]),
        tool_turn(vec![
            (
                "edit_file",
                json!({
                    "path": "pagination.py",
                    "old_text": "start = page * page_size",
                    "new_text": "start = (page - 1) * page_size",
                }),
            ),
            (
                "edit_file",
                json!({
                    "path": "pagination.py",
                    "old_text": "return len(items) // page_size",
                    "new_text": "return (len(items) + page_size - 1) // page_size",
                }),
            ),
        ]),
        tool_turn(vec![(
            "shell",
            json!({ "command": "python3 -c 'import pagination'" }),
        )]),
        final_turn("Fixed both problems."),
    ])
}

/// A script that looks around and gives up without changing anything.
fn idle_script() -> ScriptedProvider {
    ScriptedProvider::new(vec![
        tool_turn(vec![("list_files", json!({}))]),
        final_turn("I could not find the problem."),
    ])
}

#[tokio::test]
async fn a_correct_fix_is_scored_as_solved() {
    let report = run("fixing", fixing_script()).await;

    let result = &report.results[0];

    assert!(result.solved);
    assert!(result.error.is_none());
    assert_eq!(result.steps, 4);
    assert_eq!(result.usage.input_tokens, 400);
}

#[tokio::test]
async fn an_unchanged_project_is_scored_as_failed() {
    let report = run("idle", idle_script()).await;

    assert!(!report.results[0].solved);
    assert_eq!(report.totals().solve_rate(), 0.0);
}

#[tokio::test]
async fn a_provider_failure_is_an_error_not_a_failed_task() {
    let report = run("broken", ScriptedProvider::new(Vec::new())).await;

    let result = &report.results[0];

    assert!(result.error.is_some());
    assert!(!result.solved);
    assert_eq!(report.totals().errored, 1);
}

#[tokio::test]
async fn comparison_reports_an_improvement() {
    let baseline = run("idle", idle_script()).await;
    let candidate = run("fixing", fixing_script()).await;

    let comparison = Comparison::between(&baseline, &candidate);

    assert_eq!(comparison.verdict, Verdict::Improved);
    assert_eq!(comparison.gained.len(), 1);
    assert!(comparison.lost.is_empty());
    assert!(!comparison.confident);
}
