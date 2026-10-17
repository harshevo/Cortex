//! A stand-in model for trying the app without a network or a key.
//!
//! It works on a small throwaway project and always does the same task:
//! find and fix one bug. The tool calls are real and run for real; only
//! the "model" is scripted, and so are its token counts.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use cortex_core::{
    Block, Message, ModelProvider, ModelRequest, ModelResponse, ProviderError, Role, StopReason,
    StreamObserver, ToolCall, Usage,
};
use serde_json::{Value, json};
use tempfile::TempDir;

const BUGGY_LINE: &str = "    for index in range(1, window):";
const FIXED_LINE: &str = "    for index in range(1, window + 1):";

const RSI_FILE: &str = "indicators/rsi.py";

/// The demo project's own check, which the verification gate runs.
pub const CHECK: &str = "python3 tests/test_rsi.py";

const RSI_SOURCE: &str = r#""""Relative strength index."""


def average_moves(prices, window):
    """Average gain and loss over the first `window` price changes."""
    gains = 0.0
    losses = 0.0

    for index in range(1, window):
        change = prices[index] - prices[index - 1]

        if change > 0:
            gains += change
        else:
            losses -= change

    return gains / window, losses / window


def rsi(prices, window=14):
    gain, loss = average_moves(prices, window)

    if loss == 0:
        return 100.0

    return 100.0 - 100.0 / (1.0 + gain / loss)
"#;

const TEST_SOURCE: &str = r#"import sys

sys.path.insert(0, ".")

from indicators.rsi import average_moves

gain, loss = average_moves([1, 2, 3, 2, 4], window=4)

assert (gain, loss) == (1.0, 0.25), (gain, loss)

print("1 passed")
"#;

const SUMMARY: &str = "The RSI window was off by one.\n\n\
    ## What changed\n\n\
    `average_moves` in `indicators/rsi.py` covers all `window` price changes:\n\n\
    ```python\nfor index in range(1, window + 1):\n```\n\n\
    `tests/test_rsi.py` passes.";

/// Simulated generation speed, in tokens per second, for each step.
const SPEEDS: [f32; 6] = [61.0, 48.0, 55.0, 39.0, 66.0, 52.0];

/// Creates the project the demo works on.
pub fn create_project() -> Result<TempDir> {
    let project = tempfile::Builder::new()
        .prefix("cortex-demo-")
        .tempdir()
        .context("cannot create the demo project")?;

    let files = [
        (RSI_FILE, RSI_SOURCE),
        ("indicators/__init__.py", ""),
        ("tests/test_rsi.py", TEST_SOURCE),
        (
            "README.md",
            "# tradelyzer-mini\n\nA tiny project for the Cortex demo.\n",
        ),
    ];

    for (name, content) in files {
        let path = project.path().join(name);

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        fs::write(&path, content).with_context(|| format!("cannot write {}", path.display()))?;
    }

    Ok(project)
}

pub struct DemoProvider {
    project: PathBuf,

    /// The current run asked for the fix (which the person may refuse).
    asked_to_edit: AtomicBool,
}

impl DemoProvider {
    pub fn new(project: &Path) -> Self {
        Self {
            project: project.to_path_buf(),
            asked_to_edit: AtomicBool::new(false),
        }
    }

    fn bug_is_present(&self) -> bool {
        fs::read_to_string(self.project.join(RSI_FILE))
            .map(|source| source.contains(BUGGY_LINE))
            .unwrap_or(false)
    }

    /// The scripted turn for the `step`th model call of a run.
    fn turn(&self, step: usize) -> Turn {
        match step {
            0 => Turn::tools(
                "I'll start by looking at how the project is laid out.",
                vec![plan(0), ("list_files", json!({ "path": "." }))],
            ),

            // The second call guesses at a file that does not exist, as
            // models do; the error goes back and the loop carries on.
            1 => Turn::tools(
                "The indicator code is the place to look. Searching for where the window is used.",
                vec![
                    ("search", json!({ "pattern": "window" })),
                    ("read_file", json!({ "path": "indicators/utils.py" })),
                ],
            ),

            2 => Turn::tools("", vec![("read_file", json!({ "path": RSI_FILE }))]),

            3 => self.fix_or_confirm(),

            4 if self.asked_to_edit.load(Ordering::Relaxed) => self.after_the_edit(),

            _ => Turn::last(SUMMARY),
        }
    }
}

impl DemoProvider {
    fn fix_or_confirm(&self) -> Turn {
        let buggy = self.bug_is_present();

        self.asked_to_edit.store(buggy, Ordering::Relaxed);

        if buggy {
            Turn::tools(
                "Found it. `average_moves` loops over `range(1, window)`, which covers only \
                 `window - 1` price changes but divides by `window`. The last change is dropped.",
                vec![
                    plan(1),
                    (
                        "edit_file",
                        json!({ "path": RSI_FILE, "old_text": BUGGY_LINE, "new_text": FIXED_LINE }),
                    ),
                ],
            )
        } else {
            Turn::tools(
                "The loop already covers all `window` price changes, so this was fixed \
                 earlier. I'll run the test to confirm.",
                vec![run_the_test()],
            )
        }
    }

    fn after_the_edit(&self) -> Turn {
        if self.bug_is_present() {
            Turn::last(
                "The edit was not applied, so the bug in `indicators/rsi.py` is still there. \
                 Allow the edit and ask again to fix it.",
            )
        } else {
            Turn::tools(
                "Now the test, to check the fix.",
                vec![plan(2), run_the_test()],
            )
        }
    }
}

/// The plan, with the steps before `step` done and `step` under way.
fn plan(step: usize) -> (&'static str, Value) {
    let steps: Vec<Value> = ["Find the bug", "Fix the window loop", "Run the RSI test"]
        .iter()
        .enumerate()
        .map(|(index, text)| {
            let status = match index.cmp(&step) {
                std::cmp::Ordering::Less => "completed",
                std::cmp::Ordering::Equal => "in_progress",
                std::cmp::Ordering::Greater => "pending",
            };

            json!({ "step": text, "status": status })
        })
        .collect();

    (
        "update_plan",
        json!({
            "goal": "Fix the RSI average",
            "steps": steps,
            "acceptance": ["tests/test_rsi.py passes"],
        }),
    )
}

fn run_the_test() -> (&'static str, Value) {
    ("shell", json!({ "command": CHECK }))
}

struct Turn {
    text: String,
    calls: Vec<(&'static str, Value)>,
}

impl Turn {
    fn tools(text: &str, calls: Vec<(&'static str, Value)>) -> Self {
        Self {
            text: text.to_string(),
            calls,
        }
    }

    fn last(text: &str) -> Self {
        Self::tools(text, Vec::new())
    }

    fn into_message(self, step: usize) -> Message {
        let mut blocks = Vec::new();

        if !self.text.is_empty() {
            blocks.push(Block::Text { text: self.text });
        }

        for (index, (name, input)) in self.calls.into_iter().enumerate() {
            blocks.push(Block::ToolCall(ToolCall {
                id: format!("demo_{step}_{index}"),
                name: name.to_string(),
                input,
            }));
        }

        Message {
            role: Role::Assistant,
            blocks,
        }
    }
}

#[async_trait]
impl ModelProvider for DemoProvider {
    fn name(&self) -> &str {
        "demo"
    }

    fn model(&self) -> &str {
        "scripted-demo"
    }

    async fn complete(&self, request: ModelRequest<'_>) -> Result<ModelResponse, ProviderError> {
        self.complete_streaming(request, &Unwatched).await
    }

    async fn complete_streaming(
        &self,
        request: ModelRequest<'_>,
        observer: &dyn StreamObserver,
    ) -> Result<ModelResponse, ProviderError> {
        let step = steps_into_run(request.messages);
        let turn = self.turn(step);

        let stop_reason = if turn.calls.is_empty() {
            StopReason::EndTurn
        } else {
            StopReason::ToolUse
        };

        let prompt_tokens = estimate_tokens(request.system) + conversation_tokens(request.messages);
        let output_tokens = 40 + estimate_tokens(&turn.text) + 30 * turn.calls.len() as u64;

        // The first call of a run finds nothing in the cache. After that,
        // everything but the newest part of the conversation is a hit.
        let cached = if step == 0 { 0 } else { prompt_tokens * 4 / 5 };

        let usage = Usage {
            input_tokens: prompt_tokens - cached,
            output_tokens,
            cache_read_tokens: cached,
            cache_write_tokens: 0,
        };

        let speed = SPEEDS[step % SPEEDS.len()];
        let writing_time = Duration::from_secs_f32(output_tokens as f32 / speed);

        write_out(&turn.text, writing_time, observer).await;

        Ok(ModelResponse {
            message: turn.into_message(step),
            stop_reason,
            usage,
        })
    }
}

/// An observer for when nobody is watching.
struct Unwatched;

impl StreamObserver for Unwatched {
    fn text(&self, _piece: &str) {}
}

/// What the stand-in model "thinks" before each response.
const THOUGHT: &str = "Looking at what the last step returned. The task is not done until the \
                       test passes, so the next thing is whichever step gets closest to that.";

/// Hands `text` to `observer` a word at a time, spread evenly over
/// `duration`, as a model writing it would.
async fn write_out(text: &str, duration: Duration, observer: &dyn StreamObserver) {
    observer.started();

    // A real reasoning model thinks before it writes; so does this one.
    for word in THOUGHT.split_inclusive(' ') {
        tokio::time::sleep(Duration::from_millis(25)).await;

        observer.thinking(word);
    }

    let words: Vec<&str> = text.split_inclusive(' ').collect();

    if words.is_empty() {
        tokio::time::sleep(duration).await;

        return;
    }

    let pause = duration / words.len() as u32;

    for word in words {
        tokio::time::sleep(pause).await;

        observer.text(word);
    }
}

/// Model calls already made for the newest instruction.
fn steps_into_run(messages: &[Message]) -> usize {
    let is_instruction = |message: &&Message| {
        message.role == Role::User
            && message
                .blocks
                .iter()
                .any(|block| matches!(block, Block::Text { .. }))
    };

    let run_start = messages
        .iter()
        .rposition(|message| is_instruction(&message));

    messages[run_start.unwrap_or(0)..]
        .iter()
        .filter(|message| message.role == Role::Assistant)
        .count()
}

fn conversation_tokens(messages: &[Message]) -> u64 {
    messages
        .iter()
        .flat_map(|message| &message.blocks)
        .map(|block| match block {
            Block::Text { text } => estimate_tokens(text),
            Block::ToolCall(call) => estimate_tokens(&call.input.to_string()) + 12,
            Block::ToolResult(result) => estimate_tokens(&result.output) + 8,
            Block::Image(_) => 1_500,
            Block::Opaque { .. } => 0,
        })
        .sum()
}

/// The usual rule of thumb: about four characters to a token.
fn estimate_tokens(text: &str) -> u64 {
    text.len() as u64 / 4
}

#[cfg(test)]
mod tests {
    use cortex_core::ToolResult;

    use super::*;

    #[test]
    fn steps_are_counted_from_the_newest_instruction() {
        let answer = Message::assistant_text("done");

        let results = Message::tool_results(vec![ToolResult {
            call_id: "call".to_string(),
            output: "ok".to_string(),
            is_error: false,
            images: Vec::new(),
        }]);

        let first_run = vec![Message::user_text("one"), answer.clone()];

        assert_eq!(steps_into_run(&first_run[..1]), 0);
        assert_eq!(steps_into_run(&first_run), 1);

        let second_run = vec![
            Message::user_text("one"),
            answer.clone(),
            Message::user_text("two"),
            answer,
            results,
        ];

        assert_eq!(steps_into_run(&second_run), 1);
    }

    #[test]
    fn the_demo_project_has_the_bug_until_it_is_fixed() {
        let project = create_project().unwrap();
        let provider = DemoProvider::new(project.path());

        assert!(provider.bug_is_present());

        let path = project.path().join(RSI_FILE);
        let fixed = fs::read_to_string(&path)
            .unwrap()
            .replace(BUGGY_LINE, FIXED_LINE);

        fs::write(&path, fixed).unwrap();

        assert!(!provider.bug_is_present());
    }
}
