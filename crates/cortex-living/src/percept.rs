//! Reading a run's events as moments: a few short texts each.

use cortex_core::{Event, ToolCall};
use serde_json::Value;

/// Words taken from one instruction at most.
const MAX_WORDS: usize = 5;

/// Longest text kept of a command or a path.
const MAX_TEXT: usize = 48;

/// The moments in `event`, in order. Most events are one moment or none.
pub fn moments(event: &Event) -> Vec<Vec<String>> {
    match event {
        Event::RunStarted { instruction, .. } => {
            let mut percepts = vec!["person: asks".to_string()];

            percepts.extend(reaction(instruction).map(str::to_string));
            percepts.extend(
                telling_words(instruction)
                    .into_iter()
                    .map(|word| format!("word: {word}")),
            );

            vec![percepts]
        }

        Event::ModelResponded { message, .. } => {
            let calls = message.tool_calls();

            if calls.is_empty() {
                return vec![vec!["model: says it is done".to_string()]];
            }

            calls.iter().map(intent).collect()
        }

        Event::ToolFinished { tool, result, .. } => {
            let outcome = if result.is_error || exited_with_failure(tool, &result.output) {
                "result: failed"
            } else {
                "result: worked"
            };

            vec![vec![format!("did: {tool}"), outcome.to_string()]]
        }

        Event::VerificationRan { passed, .. } => {
            let outcome = if *passed {
                "checks: passed"
            } else {
                "checks: failed"
            };

            vec![vec![outcome.to_string()]]
        }

        Event::UserInterjected { text, .. } => {
            let mut percepts = vec!["person: speaks".to_string()];

            percepts.extend(reaction(text).map(str::to_string));

            vec![percepts]
        }

        Event::RunFinished { verified, .. } => {
            let how = match verified {
                Some(true) => "run: ended checked",
                Some(false) => "run: ended failing",
                None => "run: ended unchecked",
            };

            vec![vec![how.to_string()]]
        }

        _ => Vec::new(),
    }
}

/// What the model is about to do: the tool, and what it is pointed at.
fn intent(call: &ToolCall) -> Vec<String> {
    let field = |name: &str| call.input.get(name).and_then(Value::as_str);

    let mut percepts = vec![format!("does: {}", call.name)];

    if let Some(path) = field("path") {
        percepts.push(format!("file: {}", clip(path)));
    }

    if let Some(command) = field("command") {
        // The program and its first argument say which command it is;
        // the rest varies from call to call.
        let head: Vec<&str> = command.split_whitespace().take(2).collect();

        percepts.push(format!("runs: {}", clip(&head.join(" "))));
    }

    percepts
}

fn exited_with_failure(tool: &str, output: &str) -> bool {
    tool == "shell" && !output.starts_with("exit code: 0")
}

/// Whether the person is turning down or approving what was done.
fn reaction(text: &str) -> Option<&'static str> {
    let text = text.trim().to_lowercase();

    let first = text
        .split(|character: char| !character.is_alphanumeric() && character != '\'')
        .next()
        .unwrap_or_default();

    const REJECTS: &[&str] = &[
        "no", "nope", "stop", "wait", "wrong", "don't", "dont", "revert", "undo", "not",
    ];

    const ACCEPTS: &[&str] = &[
        "yes", "yep", "ok", "okay", "good", "great", "nice", "perfect", "thanks", "correct",
        "exactly",
    ];

    if REJECTS.contains(&first) || text.contains("that's wrong") || text.contains("still broken") {
        Some("person: rejects")
    } else if ACCEPTS.contains(&first) || text.contains("looks good") || text.contains("works now")
    {
        Some("person: accepts")
    } else {
        None
    }
}

/// The longer words of an instruction, lower case, in order.
fn telling_words(instruction: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();

    for piece in instruction.split(|character: char| !character.is_alphanumeric()) {
        let word = piece.to_lowercase();

        if word.len() >= 5 && !words.contains(&word) {
            words.push(word);
        }

        if words.len() == MAX_WORDS {
            break;
        }
    }

    words
}

fn clip(text: &str) -> String {
    text.chars().take(MAX_TEXT).collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_call_is_read_as_its_tool_and_target() {
        let edit = ToolCall {
            id: "1".to_string(),
            name: "edit_file".to_string(),
            input: json!({ "path": "src/fees.rs", "old_text": "a" }),
        };

        let shell = ToolCall {
            id: "2".to_string(),
            name: "shell".to_string(),
            input: json!({ "command": "cargo test -p core -- fees" }),
        };

        assert_eq!(intent(&edit), ["does: edit_file", "file: src/fees.rs"]);
        assert_eq!(intent(&shell), ["does: shell", "runs: cargo test"]);
    }

    #[test]
    fn what_the_person_says_is_read_as_a_reaction() {
        assert_eq!(reaction("no, revert that"), Some("person: rejects"));
        assert_eq!(reaction("Perfect, thanks"), Some("person: accepts"));
        assert_eq!(reaction("add a currency field"), None);
    }

    #[test]
    fn an_instruction_starts_a_moment_with_its_words() {
        let event = Event::RunStarted {
            instruction: "fix the rounding of commission fees".to_string(),
            provider: "test".to_string(),
            model: "test".to_string(),
        };

        assert_eq!(
            moments(&event),
            [["person: asks", "word: rounding", "word: commission"]]
        );
    }
}
