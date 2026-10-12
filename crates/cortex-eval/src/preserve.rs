//! Measuring whether shortening the conversation changes what the model
//! does next.
//!
//! Masking and pruning save tokens by removing old tool output. The
//! question that matters is not how much they remove but whether the
//! model, shown the shorter conversation, still takes the step it would
//! have taken with everything in front of it. That is the criterion of
//! CoACT (2026): a compression is good when it preserves the next action.
//!
//! Saved runs make this measurable without running any task. A run is
//! stopped at a step, the conversation up to there is rebuilt from the
//! log, and the model is asked for its next step twice: with the
//! conversation as it was, and with it shortened. The two answers are
//! compared.
//!
//! A model does not give the same answer twice even to the same
//! conversation, so the answer to the full conversation is also compared
//! with what the run actually did at that step. That agreement is the
//! ceiling: shortening cannot be expected to agree more often than the
//! model agrees with itself.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use cortex_core::context::{self, MaskConfig, conversation_chars};
use cortex_core::{
    Event, Message, ModelProvider, ModelRequest, ToolSpec, Workspace, prompt, replay,
};
use serde_json::Value;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Preserved {
    /// Steps at which the model was asked.
    pub samples: usize,

    /// The model took the same step with the shortened conversation as
    /// with the full one.
    pub same_when_shortened: usize,

    /// With the full conversation, the model took the step the run took:
    /// how often it agrees with itself.
    pub full_same_as_run: usize,

    /// With the shortened conversation, it took the step the run took.
    pub shortened_same_as_run: usize,

    /// Size of the sampled conversations before and after, in characters.
    pub chars_before: usize,
    pub chars_after: usize,
}

impl Preserved {
    pub fn share_removed(&self) -> f64 {
        if self.chars_before == 0 {
            return 0.0;
        }

        1.0 - self.chars_after as f64 / self.chars_before as f64
    }
}

/// One place in a saved run where shortening removes something.
struct Sample {
    full: Vec<Message>,
    shortened: Vec<Message>,

    /// What the run did next.
    taken: Action,
}

/// The step a response amounts to: the first tool it calls and what it
/// calls it on, or finishing.
#[derive(Debug, Clone, PartialEq)]
pub struct Action {
    pub tool: String,
    pub target: String,
}

impl Action {
    pub fn of(message: &Message) -> Self {
        let Some(call) = message.tool_calls().into_iter().next() else {
            return Self {
                tool: "finish".to_string(),
                target: String::new(),
            };
        };

        // The argument that says what the call is about. Line ranges and
        // limits may differ between two calls that do the same thing.
        let target = ["path", "command", "pattern", "query", "name"]
            .iter()
            .find_map(|field| call.input.get(field).and_then(Value::as_str))
            .map(str::to_string)
            .unwrap_or_else(|| call.input.to_string());

        Self {
            tool: call.name,
            target,
        }
    }
}

/// Every `.jsonl` file under `dir`, sorted.
pub fn trajectories_under(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();

    collect(dir, &mut found);

    found.sort();

    found
}

fn collect(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for path in entries.filter_map(Result::ok).map(|entry| entry.path()) {
        if path.is_dir() {
            collect(&path, found);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "jsonl")
        {
            found.push(path);
        }
    }
}

/// Asks the model at up to `per_run` steps of each saved run.
pub async fn measure(
    provider: Arc<dyn ModelProvider>,
    tools: &[ToolSpec],
    trajectories: &[PathBuf],
    mask: &MaskConfig,
    per_run: usize,
    on_sample: impl Fn(&Preserved),
) -> Result<Preserved> {
    let workspace = Workspace::new(".")?;
    let system = prompt::system_prompt(&workspace);

    let mut preserved = Preserved::default();

    for file in trajectories {
        let events = read_events(file)?;

        for sample in samples(&events, mask, per_run) {
            let full = next_action(provider.as_ref(), &system, tools, &sample.full).await?;
            let shortened =
                next_action(provider.as_ref(), &system, tools, &sample.shortened).await?;

            preserved.samples += 1;
            preserved.same_when_shortened += usize::from(full == shortened);
            preserved.full_same_as_run += usize::from(full == sample.taken);
            preserved.shortened_same_as_run += usize::from(shortened == sample.taken);
            preserved.chars_before += conversation_chars(&sample.full);
            preserved.chars_after += conversation_chars(&sample.shortened);

            on_sample(&preserved);
        }
    }

    Ok(preserved)
}

async fn next_action(
    provider: &dyn ModelProvider,
    system: &str,
    tools: &[ToolSpec],
    conversation: &[Message],
) -> Result<Action> {
    let request = ModelRequest {
        system,
        messages: conversation,
        tools,
    };

    let response = provider.complete(request).await?;

    Ok(Action::of(&response.message))
}

pub fn read_events(file: &Path) -> Result<Vec<Event>> {
    let text =
        fs::read_to_string(file).with_context(|| format!("could not read {}", file.display()))?;

    // A line written by a newer or older version that no longer parses
    // is skipped; the rest of the run is still usable.
    Ok(text
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect())
}

/// The steps of one run at which masking removes something, spread
/// evenly, at most `per_run` of them.
fn samples(events: &[Event], mask: &MaskConfig, per_run: usize) -> Vec<Sample> {
    let mut found = Vec::new();

    for (position, event) in events.iter().enumerate() {
        if !matches!(event, Event::ModelStarted { .. }) {
            continue;
        }

        // What the run did at this step is the next response in the log.
        let taken = events[position..].iter().find_map(|event| match event {
            Event::ModelResponded { message, .. } => Some(Action::of(message)),
            _ => None,
        });

        let Some(taken) = taken else {
            continue;
        };

        let full = replay::conversation(&events[..position]);

        let mut shortened = full.clone();

        if context::mask(&mut shortened, mask).results_trimmed == 0 {
            continue;
        }

        found.push(Sample {
            full,
            shortened,
            taken,
        });
    }

    if found.len() <= per_run || per_run == 0 {
        return found;
    }

    let stride = found.len() as f64 / per_run as f64;

    let keep: Vec<usize> = (0..per_run)
        .map(|pick| (pick as f64 * stride) as usize)
        .collect();

    found
        .into_iter()
        .enumerate()
        .filter(|(position, _)| keep.contains(position))
        .map(|(_, sample)| sample)
        .collect()
}

#[cfg(test)]
mod tests {
    use cortex_core::{StopReason, ToolCall, ToolResult, Usage};
    use cortex_providers::scripted::{ScriptedProvider, final_turn, tool_turn};
    use serde_json::json;

    use super::*;

    /// A run that reads `count` files, one a step, then finishes.
    fn run_events(count: u32) -> Vec<Event> {
        let mut events = vec![Event::RunStarted {
            instruction: "Fix the bug.".to_string(),
            provider: "scripted".to_string(),
            model: "scripted".to_string(),
        }];

        for step in 1..=count {
            let path = format!("file-{step}.ts");

            let call = ToolCall {
                id: "call_0".to_string(),
                name: "read_file".to_string(),
                input: json!({ "path": path }),
            };

            events.push(Event::ModelStarted { step });

            events.push(Event::ModelResponded {
                step,
                message: tool_turn(vec![("read_file", json!({ "path": path }))]).message,
                stop_reason: StopReason::ToolUse,
                usage: Usage::default(),
                duration_ms: 1,
            });

            events.push(Event::ToolStarted { step, call });

            events.push(Event::ToolFinished {
                step,
                tool: "read_file".to_string(),
                result: ToolResult {
                    call_id: "call_0".to_string(),
                    output: "x".repeat(2_000),
                    is_error: false,
                },
                duration_ms: 1,
            });
        }

        events.push(Event::ModelStarted { step: count + 1 });

        events.push(Event::ModelResponded {
            step: count + 1,
            message: final_turn("Done.").message,
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
            duration_ms: 1,
        });

        events
    }

    fn mask() -> MaskConfig {
        MaskConfig {
            keep_steps: 2,
            min_chars: 500,
            batch: 1,
        }
    }

    #[test]
    fn only_steps_where_something_is_masked_are_sampled() {
        let found = samples(&run_events(5), &mask(), 10);

        // Before step 4 there are three reads, one outside the window;
        // steps 5 and 6 follow. Earlier steps have nothing to mask.
        assert_eq!(found.len(), 3);

        assert_eq!(found[0].taken.target, "file-4.ts");
        assert_eq!(found[2].taken.tool, "finish");

        assert!(conversation_chars(&found[0].shortened) < conversation_chars(&found[0].full));

        assert_eq!(samples(&run_events(5), &mask(), 2).len(), 2);
    }

    #[tokio::test]
    async fn answers_are_compared_with_each_other_and_with_the_run() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("task-1.jsonl");

        let lines: Vec<String> = run_events(3)
            .iter()
            .map(|event| serde_json::to_string(event).unwrap())
            .collect();

        fs::write(&file, lines.join("\n")).unwrap();

        // One sample: the step after the third read, where the run
        // finished. Full conversation: the model finishes, as the run did.
        // Shortened: it reads the masked file again.
        let provider = Arc::new(ScriptedProvider::new(vec![
            final_turn("Done."),
            tool_turn(vec![("read_file", json!({ "path": "file-1.ts" }))]),
        ]));

        let preserved = measure(
            provider,
            &[],
            &trajectories_under(dir.path()),
            &mask(),
            4,
            |_| {},
        )
        .await
        .unwrap();

        assert_eq!(preserved.samples, 1);
        assert_eq!(preserved.same_when_shortened, 0);
        assert_eq!(preserved.full_same_as_run, 1);
        assert_eq!(preserved.shortened_same_as_run, 0);
        assert!(preserved.share_removed() > 0.2);
    }
}
