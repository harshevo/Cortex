use async_trait::async_trait;
use cortex_core::{Access, Tool, ToolContext, ToolError, ToolSpec};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::input;

pub struct WriteFile;

#[derive(Deserialize)]
struct Input {
    path: String,
    content: String,
}

#[async_trait]
impl Tool for WriteFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "write_file".to_string(),

            description: "Create a file, or replace the whole contents of an existing one. \
                          Missing parent directories are created. To change part of an existing \
                          file, use `edit_file` instead."
                .to_string(),

            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path, relative to the project directory.",
                    },
                    "content": {
                        "type": "string",
                        "description": "The complete new contents of the file.",
                    },
                },
                "required": ["path", "content"],
            }),
        }
    }

    fn access(&self) -> Access {
        Access::Mutating
    }

    fn describe(&self, input: &Value) -> String {
        let path = input.get("path").and_then(Value::as_str).unwrap_or("?");

        format!("write {path}")
    }

    async fn run(&self, input: Value, context: &ToolContext) -> Result<String, ToolError> {
        let input: Input = input::parse(input)?;

        let path = context.workspace.resolve(&input.path)?;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| input::io_error("create the directory for", &input.path, error))?;
        }

        std::fs::write(&path, &input.content)
            .map_err(|error| input::io_error("write", &input.path, error))?;

        let lines = input.content.lines().count();

        Ok(format!("wrote {} ({lines} lines)", input.path))
    }
}
