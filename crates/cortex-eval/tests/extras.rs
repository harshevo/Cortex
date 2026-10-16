//! The review, the lessons and repeated attempts, end to end with a
//! scripted model: the real loop, the real tools, a real project copy.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use cortex_core::{
    CheckOutcome, Event, Extras, Message, ModelProvider, ModelRequest, ModelResponse,
    ProviderError, RolloutConfig, StopReason, Usage, Verification, Verifier, Workspace,
};
use cortex_eval::preserve::read_events;
use cortex_eval::{RunOptions, Suite, SuiteReport, run_suite};
use cortex_providers::ScriptedProvider;
use cortex_providers::scripted::{final_turn, tool_turn};
use serde_json::{Value, json};

fn pagination_suite() -> Suite {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../eval/suites/smoke");

    Suite::load(&dir).unwrap().filtered(Some("fix-pagination"))
}

fn fix_start() -> (&'static str, Value) {
    (
        "edit_file",
        json!({
            "path": "pagination.py",
            "old_text": "start = page * page_size",
            "new_text": "start = (page - 1) * page_size",
        }),
    )
}

fn fix_count() -> (&'static str, Value) {
    (
        "edit_file",
        json!({
            "path": "pagination.py",
            "old_text": "return len(items) // page_size",
            "new_text": "return (len(items) + page_size - 1) // page_size",
        }),
    )
}

fn review(requirements: Value) -> ModelResponse {
    final_turn(&json!({ "requirements": requirements }).to_string())
}

/// Passes once the first bug is fixed, as a project's own test might.
struct StartIsFixed;

#[async_trait]
impl Verifier for StartIsFixed {
    fn commands(&self, _workspace: &Workspace) -> Vec<String> {
        vec!["check the first page".to_string()]
    }

    async fn verify(&self, workspace: &Workspace) -> Verification {
        let text = fs::read_to_string(workspace.root().join("pagination.py")).unwrap_or_default();

        Verification {
            checks: vec![CheckOutcome {
                command: "check the first page".to_string(),
                passed: text.contains("(page - 1) * page_size"),
                duration_ms: 1,
                output: "page 1 is empty".to_string(),
            }],
        }
    }
}

async fn run(
    provider: Arc<dyn ModelProvider>,
    change: impl FnOnce(&mut RunOptions),
) -> (SuiteReport, Vec<Event>) {
    let results = tempfile::tempdir().unwrap();

    let mut options = RunOptions {
        max_steps: 10,
        ..RunOptions::plain("extras", results.path(), cortex_tools::default_registry())
    };

    change(&mut options);

    let (report, run_dir) = run_suite(&pagination_suite(), provider, &options, |_| {})
        .await
        .unwrap();

    let events = read_events(&run_dir.join("trajectories/fix-pagination-1.jsonl")).unwrap();

    (report, events)
}

fn scripted(responses: Vec<ModelResponse>) -> Arc<dyn ModelProvider> {
    Arc::new(ScriptedProvider::new(responses))
}

#[tokio::test]
async fn the_review_sends_half_done_work_back_once() {
    let provider = scripted(vec![
        tool_turn(vec![fix_start()]),
        final_turn("Fixed the first page."),
        // The reviewer: the page count is still wrong.
        review(json!([
            { "text": "page 1 starts at the first item", "met": true, "why": "start is fixed" },
            { "text": "the page count rounds up", "met": false, "why": "page_count is unchanged" },
        ])),
        tool_turn(vec![fix_count()]),
        final_turn("Fixed the page count as well."),
    ]);

    let (report, events) = run(provider, |options| options.extras.review = true).await;

    let result = &report.results[0];

    assert!(result.solved);
    assert_eq!(result.steps, 4);

    // Five answers were paid for: four steps and the review.
    assert_eq!(result.usage.input_tokens, 500);

    let reviews: Vec<&Option<String>> = events
        .iter()
        .filter_map(|event| match event {
            Event::ReviewRan { feedback, .. } => Some(feedback),
            _ => None,
        })
        .collect();

    assert_eq!(reviews.len(), 1);
    assert!(
        reviews[0]
            .as_ref()
            .unwrap()
            .contains("the page count rounds up")
    );
}

#[tokio::test]
async fn a_second_attempt_is_made_when_the_first_fails_its_checks() {
    let provider = scripted(vec![
        // First attempt: an edit that fixes nothing.
        tool_turn(vec![(
            "edit_file",
            json!({
                "path": "pagination.py",
                "old_text": "start = page * page_size",
                "new_text": "start = page * page_size + 0",
            }),
        )]),
        final_turn("Done."),
        // Second attempt: the real fix, then its review.
        tool_turn(vec![fix_start(), fix_count()]),
        final_turn("Fixed both problems."),
        review(json!([{ "text": "pages are right", "met": true, "why": "both fixed" }])),
    ]);

    let (report, events) = run(provider, |options| {
        options.extras.rollouts = Some(RolloutConfig::new(3));
        options.verifier = Some(Arc::new(StartIsFixed));
        options.max_verification_rounds = 0;
    })
    .await;

    let result = &report.results[0];

    // The hidden check ran on the real working copy, so the second
    // attempt's changes were copied back and the first one's were not.
    assert!(result.solved);
    assert_eq!(result.verified, Some(true));
    assert_eq!(result.steps, 4);

    let started = events
        .iter()
        .filter(|event| matches!(event, Event::AttemptStarted { .. }))
        .count();

    // The third attempt was never needed.
    assert_eq!(started, 2);

    let chosen = events.iter().find_map(|event| match event {
        Event::AttemptChosen { attempt, of, .. } => Some((*attempt, *of)),
        _ => None,
    });

    assert_eq!(chosen, Some((2, 2)));

    // The kept attempt's steps are in the log as if it were the only one,
    // told how to go about the task.
    let strategy = events
        .iter()
        .any(|event| matches!(event, Event::ContextPrepared { source, .. } if source == "rollout"));

    assert!(strategy);

    let edits = events
        .iter()
        .filter(|event| matches!(event, Event::ToolFinished { tool, .. } if tool == "edit_file"))
        .count();

    assert_eq!(edits, 2);
}

/// Answers from what it is shown, so attempts made at the same time each
/// get the right answer whatever order they ask in.
struct Stateless;

#[async_trait]
impl ModelProvider for Stateless {
    fn name(&self) -> &str {
        "stateless"
    }

    fn model(&self) -> &str {
        "stateless"
    }

    async fn complete(&self, request: ModelRequest<'_>) -> Result<ModelResponse, ProviderError> {
        if request.system.starts_with("You review") {
            return Ok(review(json!([{ "text": "pages are right", "met": true }])));
        }

        let worked = request.messages.len() > 1;

        Ok(if worked {
            final_turn("Fixed both problems.")
        } else {
            tool_turn(vec![fix_start(), fix_count()])
        })
    }
}

#[tokio::test]
async fn attempts_made_at_the_same_time_all_finish_and_one_is_kept() {
    let (report, events) = run(Arc::new(Stateless), |options| {
        options.extras.rollouts = Some(RolloutConfig {
            parallel: true,
            ..RolloutConfig::new(3)
        });
    })
    .await;

    let result = &report.results[0];

    assert!(result.solved);

    // Three attempts of two steps each.
    assert_eq!(result.steps, 6);

    let finished: Vec<(u32, usize)> = events
        .iter()
        .filter_map(|event| match event {
            Event::AttemptFinished {
                attempt,
                changed_files,
                ..
            } => Some((*attempt, *changed_files)),
            _ => None,
        })
        .collect();

    assert_eq!(finished.len(), 3);
    assert!(finished.iter().all(|(_, changed)| *changed == 1));

    // Equal work: the earliest attempt is kept.
    let chosen = events.iter().find_map(|event| match event {
        Event::AttemptChosen { attempt, .. } => Some(*attempt),
        _ => None,
    });

    assert_eq!(chosen, Some(1));
}

#[tokio::test]
async fn an_attempt_cannot_touch_the_real_project_until_it_is_chosen() {
    // The model is told the project directory in its prompt. An attempt
    // must be told its own copy, or an absolute path would reach the real
    // files.
    struct Watching {
        seen: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait]
    impl ModelProvider for Watching {
        fn name(&self) -> &str {
            "watching"
        }

        fn model(&self) -> &str {
            "watching"
        }

        async fn complete(
            &self,
            request: ModelRequest<'_>,
        ) -> Result<ModelResponse, ProviderError> {
            let directory = request
                .system
                .lines()
                .find_map(|line| line.strip_prefix("Project directory: "))
                .unwrap_or_default()
                .to_string();

            self.seen.lock().unwrap().push(directory);

            Ok(ModelResponse {
                message: Message::assistant_text("Nothing to do."),
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            })
        }
    }

    let provider = Arc::new(Watching {
        seen: std::sync::Mutex::new(Vec::new()),
    });

    run(provider.clone(), |options| {
        options.extras = Extras {
            rollouts: Some(RolloutConfig::new(2)),
            ..Extras::default()
        };
    })
    .await;

    let seen = provider.seen.lock().unwrap();

    assert_eq!(seen.len(), 1);
    assert!(
        Path::new(&seen[0])
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("cortex-attempt-")
    );
}
