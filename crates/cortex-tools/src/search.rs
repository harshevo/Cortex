use async_trait::async_trait;
use cortex_core::{Access, Tool, ToolContext, ToolError, ToolSpec, Workspace};
use regex::Regex;
use serde::Deserialize;
use serde_json::{Value, json};

use cortex_index::walk;

use crate::input;

const DEFAULT_MAX_RESULTS: usize = 200;
const MAX_LINE_CHARS: usize = 300;

pub struct Search;

#[derive(Deserialize)]
struct Input {
    pattern: String,

    #[serde(default)]
    path: Option<String>,

    #[serde(default)]
    glob: Option<String>,

    #[serde(default)]
    max_results: Option<usize>,
}

#[async_trait]
impl Tool for Search {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "search".to_string(),

            description: "Search file contents with a regular expression. Returns matching \
                          lines as `path:line: text`. Use it to find definitions, call sites, \
                          and error messages before reading whole files."
                .to_string(),

            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Regular expression (Rust regex syntax).",
                    },
                    "path": {
                        "type": "string",
                        "description": "Directory or file to search. Defaults to the project directory.",
                    },
                    "glob": {
                        "type": "string",
                        "description": "Only search files whose relative path matches, e.g. `**/*.py`.",
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum number of matching lines to return (default 200).",
                    },
                },
                "required": ["pattern"],
            }),
        }
    }

    fn access(&self) -> Access {
        Access::ReadOnly
    }

    async fn run(&self, input: Value, context: &ToolContext) -> Result<String, ToolError> {
        let input: Input = input::parse(input)?;

        let workspace = context.workspace.clone();

        let matches = tokio::task::spawn_blocking(move || search(&workspace, &input))
            .await
            .map_err(|error| ToolError::Failed(error.to_string()))??;

        Ok(matches)
    }
}

fn search(workspace: &Workspace, input: &Input) -> Result<String, ToolError> {
    let regex = Regex::new(&input.pattern)
        .map_err(|error| ToolError::InvalidInput(format!("bad pattern: {error}")))?;

    let matcher = input.glob.as_deref().map(input::compile_glob).transpose()?;

    let start = workspace.resolve(input.path.as_deref().unwrap_or("."))?;
    let limit = input.max_results.unwrap_or(DEFAULT_MAX_RESULTS);

    let files = if start.is_file() {
        vec![start]
    } else {
        walk::files_under(&start)
    };

    let mut lines = Vec::new();
    let mut hit_limit = false;

    'files: for file in files {
        let relative = workspace.relative(&file);

        if matcher
            .as_ref()
            .is_some_and(|matcher| !matcher.is_match(&relative))
        {
            continue;
        }

        // Files that are not valid UTF-8 are treated as binary and skipped.
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };

        for (index, line) in text.lines().enumerate() {
            if !regex.is_match(line) {
                continue;
            }

            if lines.len() == limit {
                hit_limit = true;

                break 'files;
            }

            lines.push(format!(
                "{relative}:{}: {}",
                index + 1,
                clip(line.trim_end())
            ));
        }
    }

    if lines.is_empty() {
        return Ok("(no matches)".to_string());
    }

    let mut output = lines.join("\n");

    if hit_limit {
        output.push_str(&format!(
            "\n\n(stopped at {limit} matches; narrow the pattern, path, or glob)"
        ));
    }

    Ok(output)
}

fn clip(line: &str) -> String {
    if line.chars().count() <= MAX_LINE_CHARS {
        return line.to_string();
    }

    let clipped: String = line.chars().take(MAX_LINE_CHARS).collect();

    format!("{clipped}...")
}
