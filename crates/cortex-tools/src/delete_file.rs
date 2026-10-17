use async_trait::async_trait;
use cortex_core::{Access, Tool, ToolContext, ToolError, ToolSpec};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::input;

/// Removes a file, or a folder and everything in it when asked to.
pub struct DeleteFile;

#[derive(Deserialize)]
struct Input {
    path: String,

    #[serde(default)]
    recursive: bool,
}

#[async_trait]
impl Tool for DeleteFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "delete_file".to_string(),

            description: "Delete a file. To delete a folder and everything in it, set \
                          `recursive`. The person is always asked first."
                .to_string(),

            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File or folder path, relative to the project directory.",
                    },
                    "recursive": {
                        "type": "boolean",
                        "description": "Needed to delete a folder.",
                    },
                },
                "required": ["path"],
            }),
        }
    }

    fn access(&self) -> Access {
        Access::Mutating
    }

    fn describe(&self, input: &Value) -> String {
        let path = input.get("path").and_then(Value::as_str).unwrap_or("?");

        format!("delete {path}")
    }

    async fn run(&self, input: Value, context: &ToolContext) -> Result<String, ToolError> {
        let input: Input = input::parse(input)?;

        let path = context.workspace.resolve(&input.path)?;

        if path == context.workspace.root() {
            return Err(ToolError::InvalidInput(
                "the project directory itself cannot be deleted".to_string(),
            ));
        }

        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| input::io_error("find", &input.path, error))?;

        if metadata.is_dir() {
            if !input.recursive {
                return Err(ToolError::InvalidInput(format!(
                    "`{}` is a folder; set `recursive` to delete it and everything in it",
                    input.path
                )));
            }

            std::fs::remove_dir_all(&path)
                .map_err(|error| input::io_error("delete", &input.path, error))?;

            return Ok(format!("deleted the folder {}", input.path));
        }

        std::fs::remove_file(&path)
            .map_err(|error| input::io_error("delete", &input.path, error))?;

        Ok(format!("deleted {}", input.path))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cortex_core::{AutoApprove, Workspace};

    use super::*;

    fn context(root: &std::path::Path) -> ToolContext {
        ToolContext {
            workspace: Workspace::new(root).unwrap(),
            approver: Arc::new(AutoApprove),
        }
    }

    #[tokio::test]
    async fn a_folder_needs_recursive() {
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(project.path().join("old/inner")).unwrap();
        std::fs::write(project.path().join("old/inner/a.txt"), "a").unwrap();

        let context = context(project.path());

        let refused = DeleteFile.run(json!({ "path": "old" }), &context).await;
        assert!(refused.is_err());

        DeleteFile
            .run(json!({ "path": "old", "recursive": true }), &context)
            .await
            .unwrap();
        assert!(!project.path().join("old").exists());
    }

    #[tokio::test]
    async fn the_project_itself_and_outside_paths_are_refused() {
        let project = tempfile::tempdir().unwrap();
        let context = context(project.path());

        for path in [".", "../elsewhere"] {
            let refused = DeleteFile
                .run(json!({ "path": path, "recursive": true }), &context)
                .await;

            assert!(refused.is_err(), "{path}");
        }
    }
}
