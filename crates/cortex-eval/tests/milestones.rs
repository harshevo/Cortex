//! A long task scored by its parts, end to end with a scripted model.

use std::fs;
use std::sync::Arc;

use cortex_eval::{Comparison, RunOptions, Suite, Verdict, run_suite, validate_suite};
use cortex_providers::ScriptedProvider;
use cortex_providers::scripted::{final_turn, tool_turn};
use serde_json::json;

/// A task with two problems, each its own part: `data.txt` must contain
/// `one` and `two`.
fn suite() -> (tempfile::TempDir, Suite) {
    let dir = tempfile::tempdir().unwrap();
    let task = dir.path().join("two-problems");

    fs::create_dir_all(task.join("repo")).unwrap();
    fs::create_dir_all(task.join("solution")).unwrap();

    fs::write(task.join("repo/data.txt"), "zero\n").unwrap();
    fs::write(task.join("solution/data.txt"), "one\ntwo\n").unwrap();

    fs::write(
        task.join("task.toml"),
        r#"
instruction = "Put `one` and `two` in data.txt."

[check]
command = "grep -q one data.txt && grep -q two data.txt"

[[milestone]]
name = "one"
command = "grep -q one data.txt"

[[milestone]]
name = "two"
command = "grep -q two data.txt"
weight = 3.0
"#,
    )
    .unwrap();

    let suite = Suite::load(dir.path()).unwrap();

    (dir, suite)
}

fn writes(text: &str) -> Arc<ScriptedProvider> {
    Arc::new(ScriptedProvider::new(vec![
        tool_turn(vec![(
            "write_file",
            json!({ "path": "data.txt", "content": text }),
        )]),
        final_turn("Done."),
    ]))
}

#[tokio::test]
async fn half_the_work_scores_between_none_and_all() {
    let (_dir, suite) = suite();
    let results = tempfile::tempdir().unwrap();

    let options =
        |label: &str| RunOptions::plain(label, results.path(), cortex_tools::default_registry());

    let (nothing, _) = run_suite(&suite, writes("zero\n"), &options("nothing"), |_| {})
        .await
        .unwrap();

    let (half, _) = run_suite(&suite, writes("one\n"), &options("half"), |_| {})
        .await
        .unwrap();

    let (all, _) = run_suite(&suite, writes("one\ntwo\n"), &options("all"), |_| {})
        .await
        .unwrap();

    // The second part counts three times the first.
    assert_eq!(nothing.results[0].score(), 0.0);
    assert_eq!(half.results[0].score(), 0.25);
    assert_eq!(all.results[0].score(), 1.0);

    assert!(!half.results[0].solved);
    assert!(all.results[0].solved);

    assert!(half.to_markdown().contains("failed, 1 of 2 parts"));
    assert!(
        half.to_markdown()
            .contains("Mean share of a task done: 25%")
    );

    // Neither solved the task, and they can still be told apart.
    let comparison = Comparison::between(&nothing, &half);

    assert_eq!(comparison.verdict, Verdict::Improved);
    assert!(comparison.render().contains("Share of task done"));
}

#[tokio::test]
async fn a_part_that_passes_untouched_makes_the_task_unsound() {
    let (dir, suite) = suite();

    assert!(validate_suite(&suite).await.unwrap()[0].is_sound());

    // Now the first part is already done before the agent starts.
    fs::write(dir.path().join("two-problems/repo/data.txt"), "one\n").unwrap();

    let validation = &validate_suite(&suite).await.unwrap()[0];

    assert!(!validation.is_sound());
    assert_eq!(validation.parts_passing_untouched, ["one"]);
}
