//! The brain's senses: what happens, read as moments of a few short
//! percepts each. An agent's run is read from its events, a market from
//! its bars and what the forecasters expected of them.

use cortex_core::{Event, ToolCall};
use serde_json::Value;

/// Words taken from one instruction at most.
const MAX_WORDS: usize = 5;

/// Longest text kept of a command or a path.
const MAX_TEXT: usize = 48;

/// An instruction no longer than this is kept whole as well, so that
/// one given again and again ("run the tests") can be seen coming.
const MAX_ASKED: usize = 60;

/// The beginning of the percept that is a whole short instruction.
pub const ASKS: &str = "asks: ";

/// The percepts that are good or bad news, and how much, from -1 to 1.
pub const WORTH: [(&str, f32); 12] = [
    ("trade: won", 1.0),
    ("trade: lost", -1.0),
    ("feedback: accepted", 1.0),
    ("feedback: rejected", -1.0),
    ("checks: passed", 1.0),
    ("run: ended checked", 1.0),
    ("person: accepts", 1.0),
    ("result: worked", 0.15),
    ("result: failed", -0.3),
    ("checks: failed", -0.7),
    ("run: ended failing", -1.0),
    ("person: rejects", -1.0),
];

/// The beginnings of the percepts that are things the model does.
pub const ACTIONS: [&str; 2] = ["does: ", "runs: "];

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

            percepts.extend(asked(instruction));

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

/// An action the model is weighing, written `tool: target`, as the
/// percepts it would be met as.
/// A market's bar as percepts: which way it moved and how far by its
/// usual move, and which way each forecaster had expected it to go.
/// `forecasts` are a forecaster's name and its expected change.
pub fn bar(change: f64, usual: f64, forecasts: &[(&str, f64)]) -> Vec<String> {
    let usual = usual.abs().max(1e-12);
    let size = change.abs() / usual;
    let way = if change > 0.0 { "up" } else { "down" };
    let mut percepts = vec![if size < 0.25 {
        "move: flat".to_string()
    } else if size > 2.0 {
        format!("move: big {way}")
    } else {
        format!("move: {way}")
    }];
    for (name, expected) in forecasts {
        let name: String = name
            .chars()
            .filter(|c| c.is_alphanumeric())
            .take(16)
            .collect();
        let expects = if expected.abs() < usual * 0.25 {
            "flat"
        } else if *expected > 0.0 {
            "up"
        } else {
            "down"
        };
        percepts.push(format!("{name}: expects {expects}"));
    }
    percepts
}

/// A trade closed, as news: won or lost.
pub fn trade(profit: f64) -> Vec<String> {
    vec![
        if profit > 0.0 {
            "trade: won"
        } else {
            "trade: lost"
        }
        .to_string(),
    ]
}

pub fn weighed(action: &str) -> Vec<String> {
    let (tool, target) = action.split_once(':').unwrap_or((action, ""));
    let (tool, target) = (tool.trim(), target.trim());

    let field = if tool == "shell" { "command" } else { "path" };

    let input = if target.is_empty() {
        serde_json::json!({})
    } else {
        serde_json::json!({ field: target })
    };

    intent(&ToolCall {
        id: String::new(),
        name: tool.to_string(),
        input,
    })
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

/// A short instruction as one percept: lower case, its spaces evened out.
pub fn asked(instruction: &str) -> Option<String> {
    let text = instruction
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();

    (!text.is_empty() && text.chars().count() <= MAX_ASKED).then(|| format!("{ASKS}{text}"))
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
    fn a_bar_is_read_as_its_move_and_what_was_expected() {
        assert_eq!(
            bar(0.03, 0.01, &[("cfc", 0.02), ("spiking", -0.0001)]),
            vec!["move: big up", "cfc: expects up", "spiking: expects flat"]
        );
        assert_eq!(bar(-0.01, 0.01, &[]), vec!["move: down"]);
        assert_eq!(bar(0.0, 0.0, &[]), vec!["move: flat"]);
    }

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
    fn a_weighed_action_is_read_as_the_call_it_would_be() {
        assert_eq!(
            weighed("shell: cargo test -p core"),
            ["does: shell", "runs: cargo test"]
        );
        assert_eq!(
            weighed("edit_file: src/fees.rs"),
            ["does: edit_file", "file: src/fees.rs"]
        );
        assert_eq!(weighed("list_files"), ["does: list_files"]);
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
            [[
                "person: asks",
                "word: rounding",
                "word: commission",
                "asks: fix the rounding of commission fees"
            ]]
        );
    }
}
