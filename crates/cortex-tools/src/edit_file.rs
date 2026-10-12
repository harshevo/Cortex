use async_trait::async_trait;
use cortex_core::{Access, Tool, ToolContext, ToolError, ToolSpec};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::input;

/// Lines shown on each side of a change.
const CONTEXT_LINES: usize = 3;

/// Longest stretch of the changed file shown back.
const MAX_SHOWN_LINES: usize = 40;

#[derive(Default)]
pub struct EditFile {
    /// Answer with the changed lines as they now stand.
    show_result: bool,
}

impl EditFile {
    /// An edit that shows the lines around the change afterwards, so the
    /// model does not read the file again just to see that it worked.
    pub fn lean() -> Self {
        Self { show_result: true }
    }
}

#[derive(Deserialize)]
struct Input {
    path: String,
    old_text: String,
    new_text: String,

    #[serde(default)]
    replace_all: bool,
}

#[async_trait]
impl Tool for EditFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "edit_file".to_string(),

            description: "Replace exact text in a file. `old_text` must match the file \
                          character for character, including indentation, and must appear \
                          exactly once unless `replace_all` is true. Include enough surrounding \
                          lines to make the match unique."
                .to_string(),

            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path, relative to the project directory.",
                    },
                    "old_text": {
                        "type": "string",
                        "description": "The exact text to replace.",
                    },
                    "new_text": {
                        "type": "string",
                        "description": "The text to put in its place.",
                    },
                    "replace_all": {
                        "type": "boolean",
                        "description": "Replace every occurrence instead of requiring one.",
                    },
                },
                "required": ["path", "old_text", "new_text"],
            }),
        }
    }

    fn access(&self) -> Access {
        Access::Mutating
    }

    fn describe(&self, input: &Value) -> String {
        let path = input.get("path").and_then(Value::as_str).unwrap_or("?");

        format!("edit {path}")
    }

    async fn run(&self, input: Value, context: &ToolContext) -> Result<String, ToolError> {
        let input: Input = input::parse(input)?;

        let path = context.workspace.resolve(&input.path)?;

        let original = std::fs::read_to_string(&path)
            .map_err(|error| input::io_error("read", &input.path, error))?;

        let updated = replace(&original, &input)?;

        std::fs::write(&path, &updated)
            .map_err(|error| input::io_error("write", &input.path, error))?;

        if !self.show_result {
            return Ok(format!("edited {}", input.path));
        }

        Ok(format!(
            "edited {}\n\n{}",
            input.path,
            changed_lines(&original, &updated, &input)
        ))
    }
}

fn replace(original: &str, input: &Input) -> Result<String, ToolError> {
    if input.old_text.is_empty() {
        return Err(ToolError::InvalidInput(
            "`old_text` is empty; use `write_file` to create a file".to_string(),
        ));
    }

    if input.old_text == input.new_text {
        return Err(ToolError::InvalidInput(
            "`old_text` and `new_text` are identical".to_string(),
        ));
    }

    let occurrences = original.matches(&input.old_text).count();

    match occurrences {
        0 => Err(ToolError::Failed(format!(
            "`old_text` was not found in {}. Read the file again and copy the text exactly.",
            input.path
        ))),

        1 => Ok(original.replacen(&input.old_text, &input.new_text, 1)),

        _ if input.replace_all => Ok(original.replace(&input.old_text, &input.new_text)),

        count => Err(ToolError::Failed(format!(
            "`old_text` appears {count} times in {}. Add surrounding lines to make it unique, \
             or set `replace_all`.",
            input.path
        ))),
    }
}

/// The first changed place in `updated`, with a few lines either side,
/// numbered the way `read_file` numbers them.
fn changed_lines(original: &str, updated: &str, input: &Input) -> String {
    let Some(at) = original.find(&input.old_text) else {
        return String::new();
    };

    let first = original[..at].matches('\n').count() + 1;

    let changed = input.new_text.lines().count().max(1);

    let from = first.saturating_sub(CONTEXT_LINES).max(1);
    let shown = (changed + 2 * CONTEXT_LINES).min(MAX_SHOWN_LINES);

    let mut text = String::from("The file now reads, around the change:\n");

    for (index, line) in updated.lines().enumerate().skip(from - 1).take(shown) {
        text.push_str(&format!("{}\t{line}\n", index + 1));
    }

    let places = original.matches(&input.old_text).count();

    if places > 1 {
        text.push_str(&format!(
            "\n(the same change was made in {} other places)",
            places - 1
        ));
    }

    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_changed_lines_are_shown_with_their_numbers() {
        let original = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\n";

        let input = input("six", "SIX", false);

        let updated = replace(original, &input).unwrap();

        let shown = changed_lines(original, &updated, &input);

        assert!(
            shown.contains("3\tthree\n4\tfour\n5\tfive\n6\tSIX\n7\tseven\n8\teight\n9\tnine\n")
        );
        assert!(!shown.contains("2\ttwo"));
    }

    #[test]
    fn a_change_at_the_top_starts_at_line_one() {
        let original = "one\ntwo\nthree\n";

        let input = input("one", "ONE", false);

        let updated = replace(original, &input).unwrap();

        assert!(changed_lines(original, &updated, &input).contains("1\tONE\n2\ttwo\n"));
    }

    #[test]
    fn a_change_inside_a_line_is_shown_on_that_line() {
        let original = "a\nb\nc\nd\nlet total = price * 2;\nf\n";

        let input = input("* 2", "* 3", false);

        let updated = replace(original, &input).unwrap();

        assert!(changed_lines(original, &updated, &input).contains("5\tlet total = price * 3;\n"));
    }

    fn input(old_text: &str, new_text: &str, replace_all: bool) -> Input {
        Input {
            path: "a.txt".to_string(),
            old_text: old_text.to_string(),
            new_text: new_text.to_string(),
            replace_all,
        }
    }

    #[test]
    fn replaces_a_unique_match() {
        let updated = replace("one two three", &input("two", "2", false)).unwrap();

        assert_eq!(updated, "one 2 three");
    }

    #[test]
    fn refuses_an_ambiguous_match() {
        assert!(replace("x x", &input("x", "y", false)).is_err());
    }

    #[test]
    fn replace_all_changes_every_match() {
        let updated = replace("x x", &input("x", "y", true)).unwrap();

        assert_eq!(updated, "y y");
    }

    #[test]
    fn reports_a_missing_match() {
        assert!(replace("abc", &input("zzz", "y", false)).is_err());
    }
}
