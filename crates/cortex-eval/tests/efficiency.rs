//! End-to-end checks of the parts that save steps and tokens: carrying on
//! after a provider failure, running lookups together, the lean tools, and
//! removing superseded results. A scripted model stands in for a real one.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use cortex_core::{
    Access, Agent, AgentConfig, AutoApprove, Block, Message, ModelProvider, ModelRequest,
    ModelResponse, ProviderError, PruneConfig, Tool, ToolContext, ToolError, ToolRegistry,
    ToolSpec, Workspace,
};
use cortex_eval::{RunOptions, Suite, SuiteReport, run_suite};
use cortex_providers::ScriptedProvider;
use cortex_providers::scripted::{final_turn, tool_turn};
use serde_json::{Value, json};

/// A provider that turns away chosen requests as rate-limited and answers
/// the rest from a script.
struct Flaky {
    script: ScriptedProvider,
    requests: AtomicU32,

    /// Whether the request with this number, counted from 1, is refused.
    refuses: fn(u32) -> bool,
}

impl Flaky {
    fn new(script: ScriptedProvider, refuses: fn(u32) -> bool) -> Self {
        Self {
            script,
            requests: AtomicU32::new(0),
            refuses,
        }
    }
}

#[async_trait]
impl ModelProvider for Flaky {
    fn name(&self) -> &str {
        "flaky"
    }

    fn model(&self) -> &str {
        "scripted"
    }

    async fn complete(&self, request: ModelRequest<'_>) -> Result<ModelResponse, ProviderError> {
        let number = self.requests.fetch_add(1, Ordering::Relaxed) + 1;

        if (self.refuses)(number) {
            return Err(ProviderError::Http {
                status: 429,
                body: "at capacity".to_string(),
            });
        }

        self.script.complete(request).await
    }
}

fn suite() -> Suite {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../eval/suites/smoke");

    Suite::load(&dir).unwrap().filtered(Some("fix-pagination"))
}

/// Reads the file, fixes both bugs, runs it, and stops: four steps.
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

async fn run_flaky(
    label: &str,
    refuses: fn(u32) -> bool,
    max_resumes: u32,
) -> (SuiteReport, PathBuf, tempfile::TempDir) {
    let results = tempfile::tempdir().unwrap();

    let options = RunOptions {
        max_resumes,
        resume_pause: Duration::ZERO,
        ..RunOptions::plain(label, results.path(), cortex_tools::default_registry())
    };

    let provider = Flaky::new(fixing_script(), refuses);

    let (report, run_dir) = run_suite(&suite(), Arc::new(provider), &options, |_| {})
        .await
        .unwrap();

    (report, run_dir, results)
}

fn events(run_dir: &Path) -> Vec<Value> {
    fs::read_to_string(run_dir.join("trajectories/fix-pagination-1.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[tokio::test]
async fn a_run_carries_on_from_the_step_the_provider_failed_on() {
    // The third request fails, after the file was read and both edits made.
    let (report, run_dir, _results) = run_flaky("resumed", |number| number == 3, 5).await;

    let result = &report.results[0];

    // The attempt is scored, not thrown away, and nothing is counted twice:
    // the script has four turns and each was asked for once.
    assert!(result.error.is_none());
    assert!(result.solved);
    assert_eq!(result.resumes, 1);
    assert_eq!(result.steps, 4);
    assert_eq!(result.usage.input_tokens, 400);

    let events = events(&run_dir);

    let resumed: Vec<&Value> = events
        .iter()
        .filter(|event| event["type"] == "run_resumed")
        .collect();

    assert_eq!(resumed.len(), 1);
    assert_eq!(resumed[0]["steps"], 2);

    // One run from start to finish, as far as anything listening can tell.
    let count = |kind: &str| events.iter().filter(|event| event["type"] == kind).count();

    assert_eq!(count("run_started"), 1);
    assert_eq!(count("run_finished"), 1);

    assert!(
        report
            .to_markdown()
            .contains("Carried on after a provider failure: 1 attempts")
    );
}

#[tokio::test]
async fn an_attempt_that_keeps_failing_records_what_it_spent() {
    // Two steps are answered and then the provider refuses for good.
    let (report, _run_dir, _results) = run_flaky("outage", |number| number > 2, 2).await;

    let result = &report.results[0];

    assert!(result.error.as_deref().unwrap().contains("429"));
    assert!(!result.solved);
    assert_eq!(result.resumes, 2);
    assert_eq!(result.steps, 2);
    assert_eq!(result.usage.input_tokens, 200);
}

#[tokio::test]
async fn attempts_lost_to_an_outage_do_not_lower_the_mean() {
    let (mut report, _run_dir, _results) = run_flaky("clean", |_| false, 0).await;
    let (outage, _outage_dir, _outage_results) = run_flaky("outage", |number| number > 2, 0).await;

    report.results.extend(outage.results);

    let totals = report.totals();

    assert_eq!(totals.attempts, 2);
    assert_eq!(totals.errored, 1);

    // The scored attempt took four steps. The lost one's two steps are in
    // the totals, because they were paid for, but not in the mean.
    assert_eq!(totals.steps, 6);
    assert_eq!(totals.mean_steps(), 4.0);
    assert_eq!(totals.usage.input_tokens, 600);
}

/// A project with one long source file and one short text file.
fn project() -> (tempfile::TempDir, ToolContext) {
    let dir = tempfile::tempdir().unwrap();

    let mut long = String::new();

    for number in 0..150 {
        long.push_str(&format!(
            "def handler_{number}(value):\n    total = value + {number}\n    return total\n\n"
        ));
    }

    fs::write(dir.path().join("handlers.py"), long).unwrap();

    let mut notes = String::new();

    for number in 1..=40 {
        notes.push_str(&format!("note {number}: keep the ledger balanced\n"));
    }

    fs::write(dir.path().join("notes.txt"), notes).unwrap();

    let context = ToolContext {
        workspace: Workspace::new(dir.path()).unwrap(),
        approver: Arc::new(AutoApprove),
    };

    (dir, context)
}

fn lean_tools() -> ToolRegistry {
    let mut tools = cortex_tools::default_registry();

    cortex_tools::add_lean_tools(&mut tools);

    tools
}

fn config(prune: Option<PruneConfig>) -> AgentConfig {
    AgentConfig {
        extras: Default::default(),
        system_prompt: "test".to_string(),
        max_steps: 20,
        prune,
        max_verification_rounds: 0,
        fold_repeats: true,
    }
}

/// The tool outputs of the conversation, one list per step, in the order
/// they were given back to the model.
fn outputs(conversation: &[Message]) -> Vec<Vec<String>> {
    conversation
        .iter()
        .map(|message| {
            message
                .blocks
                .iter()
                .filter_map(|block| match block {
                    Block::ToolResult(result) => Some(result.output.clone()),
                    _ => None,
                })
                .collect::<Vec<String>>()
        })
        .filter(|outputs| !outputs.is_empty())
        .collect()
}

fn lean_script() -> ScriptedProvider {
    ScriptedProvider::new(vec![
        // Three lookups in one response.
        tool_turn(vec![
            ("read_file", json!({ "path": "handlers.py" })),
            ("read_file", json!({ "path": "notes.txt" })),
            (
                "read_file",
                json!({ "path": "handlers.py", "offset": 5, "limit": 3 }),
            ),
        ]),
        // The same file again, unchanged.
        tool_turn(vec![("read_file", json!({ "path": "notes.txt" }))]),
        tool_turn(vec![(
            "edit_file",
            json!({
                "path": "notes.txt",
                "old_text": "note 20: keep the ledger balanced",
                "new_text": "note 20: the ledger was rebalanced",
            }),
        )]),
        // And again, now that it has changed.
        tool_turn(vec![("read_file", json!({ "path": "notes.txt" }))]),
        // A passing test run with a long log. `: pytest` does nothing; it
        // is there so the command reads as a test run.
        tool_turn(vec![(
            "shell",
            json!({ "command": "seq 1 400; echo '400 passed in 1.2s'; : pytest" }),
        )]),
        final_turn("Done."),
    ])
}

#[tokio::test]
async fn the_lean_tools_return_less_and_lose_nothing() {
    let (_dir, context) = project();

    let agent = Agent::new(Arc::new(lean_script()), lean_tools(), context, config(None));

    let mut conversation = Vec::new();

    let report = agent
        .run(&mut conversation, "tidy the notes")
        .await
        .unwrap();

    assert_eq!(report.steps, 6);

    let steps = outputs(&conversation);

    // Step 1: the results come back in the order the calls were made.
    let (long_file, notes, range) = (&steps[0][0], &steps[0][1], &steps[0][2]);

    // The long file, asked for whole, is answered with its outline.
    assert!(long_file.starts_with("handlers.py has 600 lines, so this is its outline"));
    assert!(long_file.contains("def handler_149(value)"));
    assert!(!long_file.contains("total = value + 149"));
    assert!(long_file.len() < 10_000);

    // A short file is read as text, and so is a range of the long one.
    assert!(notes.starts_with("1\tnote 1: keep the ledger balanced\n"));
    assert!(range.starts_with("5\tdef handler_1(value):\n6\t    total = value + 1\n"));

    // Step 2: the unchanged file is not sent a second time.
    assert!(steps[1][0].starts_with("Unchanged."));
    assert!(steps[1][0].len() < 300);

    // Step 3: the edit shows the lines it changed, with their neighbours.
    assert!(steps[2][0].starts_with("edited notes.txt\n"));
    assert!(steps[2][0].contains("19\tnote 19: keep the ledger balanced\n"));
    assert!(steps[2][0].contains("20\tnote 20: the ledger was rebalanced\n"));
    assert!(!steps[2][0].contains("note 30"));

    // Step 4: after the edit the file is shown in full again.
    assert!(steps[3][0].contains("20\tnote 20: the ledger was rebalanced\n"));
    assert!(steps[3][0].contains("40\tnote 40: keep the ledger balanced\n"));

    // Step 5: the passing run keeps its status and summary, not its log.
    assert!(steps[4][0].starts_with("exit code: 0\n"));
    assert!(steps[4][0].contains("earlier lines left out because the command succeeded"));
    assert!(steps[4][0].ends_with("400 passed in 1.2s"));
    assert!(steps[4][0].len() < 700);
}

#[tokio::test]
async fn a_read_made_stale_by_an_edit_is_removed_and_the_rest_kept() {
    let (_dir, context) = project();

    let prune = PruneConfig {
        min_chars: 500,
        min_reclaim_chars: 500,
    };

    let agent = Agent::new(
        Arc::new(lean_script()),
        lean_tools(),
        context,
        config(Some(prune)),
    );

    let mut conversation = Vec::new();

    agent
        .run(&mut conversation, "tidy the notes")
        .await
        .unwrap();

    let steps = outputs(&conversation);

    // The first read of the notes describes the file before the edit, and
    // the read in step 4 replaced it.
    assert!(steps[0][1].starts_with("[This older result was removed"));
    assert!(steps[3][0].contains("20\tnote 20: the ledger was rebalanced\n"));

    // Results that nothing replaced are untouched, however old.
    assert!(steps[0][0].starts_with("handlers.py has 600 lines"));
    assert!(steps[0][2].starts_with("5\tdef handler_1(value):"));
}

/// A lookup that takes a while, to show whether lookups run together.
struct SlowLookup;

#[async_trait]
impl Tool for SlowLookup {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "slow_lookup".to_string(),
            description: "Looks something up, slowly.".to_string(),
            input_schema: json!({ "type": "object", "properties": {} }),
        }
    }

    fn access(&self) -> Access {
        Access::ReadOnly
    }

    async fn run(&self, input: Value, _context: &ToolContext) -> Result<String, ToolError> {
        tokio::time::sleep(Duration::from_millis(300)).await;

        Ok(format!("looked up {}", input["name"]))
    }
}

#[tokio::test]
async fn lookups_asked_for_together_run_together() {
    let (_dir, context) = project();

    let mut tools = cortex_tools::default_registry();

    tools.register(SlowLookup);

    let script = ScriptedProvider::new(vec![
        tool_turn(vec![
            ("slow_lookup", json!({ "name": "a" })),
            ("slow_lookup", json!({ "name": "b" })),
            ("slow_lookup", json!({ "name": "c" })),
            ("slow_lookup", json!({ "name": "d" })),
        ]),
        final_turn("Done."),
    ]);

    let agent = Agent::new(Arc::new(script), tools, context, config(None));

    let mut conversation = Vec::new();

    let started = Instant::now();

    agent
        .run(&mut conversation, "look things up")
        .await
        .unwrap();

    // One after another these would take 1.2 seconds.
    assert!(started.elapsed() < Duration::from_millis(900));

    assert_eq!(
        outputs(&conversation)[0],
        [
            "looked up \"a\"",
            "looked up \"b\"",
            "looked up \"c\"",
            "looked up \"d\""
        ]
    );
}
