use async_trait::async_trait;
use cortex_core::{Access, Tool, ToolContext, ToolError, ToolSpec, Workspace};
use serde::Deserialize;
use serde_json::{Value, json};

use cortex_index::walk;

use crate::input;

const MAX_RESULTS: usize = 1_000;

pub struct ListFiles;

#[derive(Deserialize)]
struct Input {
    #[serde(default)]
    path: Option<String>,

    #[serde(default)]
    pattern: Option<String>,
}

#[async_trait]
impl Tool for ListFiles {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "list_files".to_string(),

            description: "List files in the project, skipping anything ignored by .gitignore. \
                          Optionally restrict to a directory and to a glob such as `**/*.py` or \
                          `src/**/test_*.rs`."
                .to_string(),

            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Directory to list. Defaults to the project directory.",
                    },
                    "pattern": {
                        "type": "string",
                        "description": "Glob matched against each path relative to the project directory.",
                    },
                },
            }),
        }
    }

    fn access(&self) -> Access {
        Access::ReadOnly
    }

    async fn run(&self, input: Value, context: &ToolContext) -> Result<String, ToolError> {
        let input: Input = input::parse(input)?;

        let workspace = context.workspace.clone();

        let listing = tokio::task::spawn_blocking(move || list(&workspace, &input))
            .await
            .map_err(|error| ToolError::Failed(error.to_string()))??;

        Ok(listing)
    }
}

fn list(workspace: &Workspace, input: &Input) -> Result<String, ToolError> {
    let start = workspace.resolve(input.path.as_deref().unwrap_or("."))?;

    if !start.is_dir() {
        return Err(ToolError::Failed(format!(
            "`{}` is not a directory",
            workspace.relative(&start)
        )));
    }

    let matcher = input
        .pattern
        .as_deref()
        .map(input::compile_glob)
        .transpose()?;

    let paths: Vec<String> = walk::files_under(&start)
        .iter()
        .map(|path| workspace.relative(path))
        .filter(|relative| {
            matcher
                .as_ref()
                .is_none_or(|matcher| matcher.is_match(relative))
        })
        .collect();

    if paths.is_empty() {
        return Ok("(no files found)".to_string());
    }

    let shown = paths.len().min(MAX_RESULTS);

    let mut output = paths[..shown].join("\n");

    if paths.len() > shown {
        output.push_str(&format!(
            "\n\n(showing {shown} of {} files; narrow the path or pattern)",
            paths.len()
        ));
    }

    Ok(output)
}
