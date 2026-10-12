use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use cortex_core::{Access, Tool, ToolContext, ToolError, ToolSpec};
use cortex_index::CodeIndex;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::input;
use crate::outline;

const DEFAULT_LIMIT: usize = 2_000;

/// A file longer than this is answered with its outline when it is asked
/// for whole. In the measured baseline the ten largest reads were half of
/// everything `read_file` returned, and each was re-sent on every later
/// step.
const OUTLINE_ABOVE_LINES: usize = 400;

const PLAIN_DESCRIPTION: &str = "Read a text file. Returns the lines prefixed with their line \
                                 numbers. Reads up to 2000 lines; use `offset` and `limit` for \
                                 a specific range of a larger file.";

const LEAN_DESCRIPTION: &str = "Read a text file. Returns the lines prefixed with their line \
                                numbers. A source file longer than 400 lines, asked for without \
                                `offset` or `limit`, is answered with its outline (every \
                                function, class and type with its line range) instead of its \
                                text; read the parts you need with `offset` and `limit`, \
                                several ranges in one response if you need several. Give \
                                `limit` to get the text of a long file anyway.";

#[derive(Default)]
pub struct ReadFile {
    /// Used to outline long files. Without it every file is read as text.
    index: Option<Arc<CodeIndex>>,
}

impl ReadFile {
    /// A reader that answers with an outline when asked for the whole of
    /// a long source file.
    pub fn lean(index: Arc<CodeIndex>) -> Self {
        Self { index: Some(index) }
    }

    /// The outline to show in place of the file, when that applies.
    fn outline_instead(&self, input: &Input, path: &Path, text: &str) -> Option<String> {
        let index = self.index.as_ref()?;

        if input.offset.is_some() || input.limit.is_some() {
            return None;
        }

        let total = text.lines().count();

        if total <= OUTLINE_ABOVE_LINES {
            return None;
        }

        let symbols = index.file_symbols(path);

        if symbols.is_empty() {
            return None;
        }

        let mut output = format!(
            "{} has {total} lines, so this is its outline, not its text. Read the parts you \
             need with `offset` and `limit`.\n\n",
            input.path
        );

        for symbol in &symbols {
            output.push_str(&outline::render(symbol));
        }

        Some(output)
    }
}

#[derive(Deserialize)]
struct Input {
    path: String,

    /// First line to return, starting at 1.
    #[serde(default)]
    offset: Option<usize>,

    #[serde(default)]
    limit: Option<usize>,
}

#[async_trait]
impl Tool for ReadFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read_file".to_string(),

            description: match self.index {
                Some(_) => LEAN_DESCRIPTION,
                None => PLAIN_DESCRIPTION,
            }
            .to_string(),

            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path, relative to the project directory.",
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Line number to start from (1 is the first line).",
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of lines to return.",
                    },
                },
                "required": ["path"],
            }),
        }
    }

    fn access(&self) -> Access {
        Access::ReadOnly
    }

    async fn run(&self, input: Value, context: &ToolContext) -> Result<String, ToolError> {
        let input: Input = input::parse(input)?;

        let path = context.workspace.resolve(&input.path)?;

        let text = std::fs::read_to_string(&path)
            .map_err(|error| input::io_error("read", &input.path, error))?;

        if let Some(outline) = self.outline_instead(&input, &path, &text) {
            return Ok(outline);
        }

        let first = input.offset.unwrap_or(1).max(1);
        let limit = input.limit.unwrap_or(DEFAULT_LIMIT);

        Ok(number_lines(&text, first, limit))
    }
}

fn number_lines(text: &str, first: usize, limit: usize) -> String {
    let total = text.lines().count();

    if total == 0 {
        return "(the file is empty)".to_string();
    }

    let mut output = String::new();

    for (index, line) in text.lines().enumerate().skip(first - 1).take(limit) {
        output.push_str(&format!("{}\t{line}\n", index + 1));
    }

    let last = (first - 1 + limit).min(total);

    if first > total {
        output = format!("(line {first} is past the end; the file has {total} lines)");
    } else if first > 1 || last < total {
        output.push_str(&format!("\n(showing lines {first}-{last} of {total})"));
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_every_line() {
        assert_eq!(number_lines("a\nb\n", 1, 10), "1\ta\n2\tb\n");
    }

    #[test]
    fn reports_the_range_when_partial() {
        let output = number_lines("a\nb\nc\nd\n", 2, 2);

        assert!(output.starts_with("2\tb\n3\tc\n"));
        assert!(output.ends_with("(showing lines 2-3 of 4)"));
    }
}
