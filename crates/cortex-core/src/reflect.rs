//! Taking lessons from a finished run.
//!
//! An episode records what happened. A lesson says what to do next time:
//! a strategy that worked, or a mistake and how it was got out of. Lessons
//! are taken from runs that failed as well as runs that passed, because
//! the failures are where most of what is worth knowing is.
//!
//! The lessons are written by the model from a digest of the run. They
//! are stored by memory, which hears them as an event.

use serde::{Deserialize, Serialize};

use crate::agent::Outcome;
use crate::error::ProviderError;
use crate::judge;
use crate::message::{Block, Message, Role};
use crate::provider::ModelProvider;
use crate::usage::Usage;

/// Lessons kept from one run at most.
const MAX_LESSONS: usize = 3;

const MAX_DIGEST_CHARS: usize = 9_000;
const MAX_INSTRUCTION_CHARS: usize = 3_000;

/// How much of one call's input and of one failure is quoted.
const MAX_INPUT_CHARS: usize = 160;
const MAX_ERROR_CHARS: usize = 400;

const SYSTEM: &str = "\
You read the record of one run of a coding agent and write down what a \
future run in the same repository should know. Write at most three lessons, \
and none if the run taught nothing that the code itself would not show.

A good lesson is specific and reusable: where a kind of problem lives in \
this repository, which command checks it, which approach failed and what \
worked instead. Do not restate the task, and do not record what was changed; \
that is kept elsewhere.

Answer with JSON only, in this shape:

{\"lessons\": [{\"title\": \"a few words\", \"when\": \"the situation it applies \
to\", \"advice\": \"what to do, one or two sentences\", \"paths\": [\"files it is \
about\"]}]}";

/// One thing a run taught.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lesson {
    pub title: String,

    /// The situation the lesson applies to.
    pub when: String,

    /// What to do in it.
    pub advice: String,

    /// Files the lesson is about. If one is gone the lesson is stale.
    #[serde(default)]
    pub paths: Vec<String>,

    /// Whether the run it came from ended with failing checks or without
    /// finishing.
    #[serde(default)]
    pub from_failure: bool,
}

#[derive(Deserialize)]
struct Parsed {
    lessons: Vec<Lesson>,
}

/// What the run looked like from outside, for the model to read.
///
/// Every call is one line; failures are quoted, because the way out of a
/// failure is what a lesson is usually about.
pub fn digest(conversation: &[Message]) -> String {
    let mut text = String::new();
    let mut step = 0;

    for message in conversation {
        match message.role {
            Role::Assistant => {
                let calls = message.tool_calls();

                if calls.is_empty() {
                    continue;
                }

                step += 1;

                for call in calls {
                    let input = clip(&call.input.to_string(), MAX_INPUT_CHARS);

                    text.push_str(&format!("{step}. {} {input}\n", call.name));
                }
            }

            Role::User => {
                for block in &message.blocks {
                    match block {
                        Block::ToolResult(result) if result.is_error => {
                            let error = clip(&result.output, MAX_ERROR_CHARS);

                            text.push_str(&format!("   failed: {error}\n"));
                        }

                        // Text after the first message is the harness
                        // speaking: failed checks, a review, advice.
                        Block::Text { text: said } if step > 0 => {
                            let said = clip(said, MAX_ERROR_CHARS);

                            text.push_str(&format!("   harness: {said}\n"));
                        }

                        _ => {}
                    }
                }
            }
        }
    }

    judge::clip_middle(&text, MAX_DIGEST_CHARS)
}

/// Asks the model what the run taught.
pub async fn lessons(
    provider: &dyn ModelProvider,
    instruction: &str,
    conversation: &[Message],
    outcome: Outcome,
    verified: Option<bool>,
) -> Result<(Vec<Lesson>, Usage), ProviderError> {
    let failed = outcome != Outcome::Completed || verified == Some(false);

    let ending = match (outcome, verified) {
        (Outcome::Completed, Some(true)) => "finished, and the project's checks passed",
        (Outcome::Completed, Some(false)) => "finished, but the project's checks still failed",
        (Outcome::Completed, None) => "finished; no checks were run on the result",
        (Outcome::StepLimit, _) => "ran out of steps before finishing",
        (Outcome::Truncated | Outcome::Refused, _) => "stopped before finishing",
    };

    let question = format!(
        "## Task\n\n{}\n\n## What the agent did, in order\n\n{}\n## How it ended\n\nThe run {ending}.",
        judge::clip_middle(instruction, MAX_INSTRUCTION_CHARS),
        digest(conversation),
    );

    let answer = judge::ask(provider, SYSTEM, &question).await?;

    let mut lessons = answer
        .json()
        .and_then(|value| serde_json::from_value::<Parsed>(value).ok())
        .map(|parsed| parsed.lessons)
        .unwrap_or_default();

    lessons.retain(|lesson| !lesson.advice.trim().is_empty());
    lessons.truncate(MAX_LESSONS);

    for lesson in &mut lessons {
        lesson.from_failure = failed;
    }

    Ok((lessons, answer.usage))
}

fn clip(text: &str, max_chars: usize) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");

    if line.chars().count() <= max_chars {
        return line;
    }

    let start: String = line.chars().take(max_chars).collect();

    format!("{start}...")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::message::{ToolCall, ToolResult};

    #[test]
    fn the_digest_keeps_calls_failures_and_what_the_harness_said() {
        let call = |name: &str, input| Message {
            role: Role::Assistant,
            blocks: vec![Block::ToolCall(ToolCall {
                id: "call_0".to_string(),
                name: name.to_string(),
                input,
            })],
        };

        let result = |output: &str, is_error| {
            Message::tool_results(vec![ToolResult {
                call_id: "call_0".to_string(),
                output: output.to_string(),
                is_error,
            }])
        };

        let conversation = vec![
            Message::user_text("Fix the rounding."),
            call("read_file", json!({ "path": "money.ts" })),
            result(&"x".repeat(5_000), false),
            call("shell", json!({ "command": "npm test" })),
            result("3 tests failed:\n  rounding", true),
            Message::assistant_text("Done."),
            Message::user_text("The harness ran the checks and they do not pass."),
        ];

        let digest = digest(&conversation);

        assert_eq!(
            digest,
            "1. read_file {\"path\":\"money.ts\"}\n\
             2. shell {\"command\":\"npm test\"}\n   \
             failed: 3 tests failed: rounding\n   \
             harness: The harness ran the checks and they do not pass.\n"
        );
    }
}
