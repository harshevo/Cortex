use async_trait::async_trait;
use cortex_core::{Access, Tool, ToolContext, ToolError, ToolSpec};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::input;

/// Moves or renames a file or folder inside the project.
pub struct MoveFile;

#[derive(Deserialize)]
struct Input {
    from: String,
    to: String,
}

#[async_trait]
impl Tool for MoveFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "move_file".to_string(),

            description: "Move or rename a file or folder. Missing parent directories of the \
                          destination are created. An existing destination is not replaced."
                .to_string(),

            input_schema: json!({
                "type": "object",
                "properties": {
                    "from": {
                        "type": "string",
                        "description": "Current path, relative to the project directory.",
                    },
                    "to": {
                        "type": "string",
                        "description": "New path, relative to the project directory.",
                    },
                },
                "required": ["from", "to"],
            }),
        }
    }

    fn access(&self) -> Access {
        Access::Mutating
    }

    fn describe(&self, input: &Value) -> String {
        let from = input.get("from").and_then(Value::as_str).unwrap_or("?");
        let to = input.get("to").and_then(Value::as_str).unwrap_or("?");

        format!("move {from} to {to}")
    }

    async fn run(&self, input: Value, context: &ToolContext) -> Result<String, ToolError> {
        let input: Input = input::parse(input)?;

        let from = context.workspace.resolve(&input.from)?;
        let to = context.workspace.resolve(&input.to)?;

        if !from.exists() {
            return Err(ToolError::InvalidInput(format!(
                "`{}` does not exist",
                input.from
            )));
        }

        if to.exists() {
            return Err(ToolError::InvalidInput(format!(
                "`{}` already exists; delete it first if it should be replaced",
                input.to
            )));
        }

        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| input::io_error("create the directory for", &input.to, error))?;
        }

        std::fs::rename(&from, &to).map_err(|error| input::io_error("move", &input.from, error))?;

        Ok(format!("moved {} to {}", input.from, input.to))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cortex_core::{AutoApprove, Workspace};

    use super::*;

    #[tokio::test]
    async fn moves_into_new_folders_and_never_overwrites() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();

        std::fs::write(root.join("a.txt"), "a").unwrap();
        std::fs::write(root.join("b.txt"), "b").unwrap();

        let context = ToolContext {
            workspace: Workspace::new(root).unwrap(),
            approver: Arc::new(AutoApprove),
        };

        MoveFile
            .run(json!({ "from": "a.txt", "to": "x/y/a.txt" }), &context)
            .await
            .unwrap();
        assert!(root.join("x/y/a.txt").exists());

        let refused = MoveFile
            .run(json!({ "from": "x/y/a.txt", "to": "b.txt" }), &context)
            .await;
        assert!(refused.is_err());
        assert_eq!(std::fs::read_to_string(root.join("b.txt")).unwrap(), "b");
    }
}
