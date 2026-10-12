use std::sync::Arc;

use async_trait::async_trait;
use cortex_core::{Access, Tool, ToolContext, ToolError, ToolSpec};
use cortex_index::{CodeIndex, Symbol};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::input;

/// Lists the functions, classes and types in a file with their line ranges.
pub struct Outline {
    index: Arc<CodeIndex>,
}

impl Outline {
    pub fn new(index: Arc<CodeIndex>) -> Self {
        Self { index }
    }
}

#[derive(Deserialize)]
struct Input {
    path: String,
}

#[async_trait]
impl Tool for Outline {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "outline".to_string(),

            description: "List the functions, classes, methods and types defined in a source \
                          file, each with its line range. Much cheaper than reading the file. \
                          Use it on any file longer than about 150 lines to decide which part \
                          to read, then read that part with `read_symbol` or with `read_file` \
                          and a line range. Supports Python, TypeScript, JavaScript and Rust."
                .to_string(),

            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path, relative to the project directory.",
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

        let source = std::fs::read_to_string(&path)
            .map_err(|error| input::io_error("read", &input.path, error))?;

        let symbols = self.index.file_symbols(&path);

        let total_lines = source.lines().count();

        if symbols.is_empty() {
            return Ok(format!(
                "{} has {total_lines} lines and no symbols the index recognises. \
                 Read it with `read_file`.",
                input.path
            ));
        }

        let mut output = format!("{} ({total_lines} lines)\n", input.path);

        for symbol in &symbols {
            output.push_str(&render(symbol));
        }

        Ok(output)
    }
}

pub(crate) fn render(symbol: &Symbol) -> String {
    let indent = "  ".repeat(symbol.depth + 1);

    format!(
        "{indent}{}-{}  {}  {}\n",
        symbol.start_line,
        symbol.end_line,
        symbol.kind.label(),
        symbol.signature,
    )
}
