use std::sync::Arc;

use async_trait::async_trait;
use cortex_core::{Access, Tool, ToolContext, ToolError, ToolSpec};
use cortex_retrieval::{Hit, SemanticIndex};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::input;

const DEFAULT_RESULTS: usize = 8;
const MAX_RESULTS: usize = 20;

/// The best matches are shown with their source so the agent often needs no
/// follow-up read; the rest are listed by location only.
const RESULTS_WITH_SOURCE: usize = 3;
const MAX_SOURCE_LINES: usize = 60;

/// Finds code by what it does rather than by the words it contains.
pub struct SemanticSearch {
    index: Arc<SemanticIndex>,
}

impl SemanticSearch {
    pub fn new(index: Arc<SemanticIndex>) -> Self {
        Self { index }
    }
}

#[derive(Deserialize)]
struct Input {
    query: String,

    #[serde(default)]
    path: Option<String>,

    #[serde(default)]
    max_results: Option<usize>,
}

#[async_trait]
impl Tool for SemanticSearch {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "semantic_search".to_string(),

            description: "Find code by meaning. Describe in plain words what the code does or \
                          the behaviour you are investigating, for example `where trades are \
                          grouped into calendar days` or `rounding of commission across \
                          trades`, and get the most relevant functions with their locations. \
                          Use this first when a task describes behaviour and you do not yet \
                          know which file or function is involved. Use `search` instead when \
                          you know an exact name or string."
                .to_string(),

            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "A description of the code or behaviour to find.",
                    },
                    "path": {
                        "type": "string",
                        "description": "Directory to search in. Defaults to the project.",
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "How many matches to return (default 8, maximum 20).",
                    },
                },
                "required": ["query"],
            }),
        }
    }

    fn access(&self) -> Access {
        Access::ReadOnly
    }

    async fn run(&self, input: Value, context: &ToolContext) -> Result<String, ToolError> {
        let input: Input = input::parse(input)?;

        if input.query.trim().is_empty() {
            return Err(ToolError::InvalidInput("`query` is empty".to_string()));
        }

        let root = context
            .workspace
            .resolve(input.path.as_deref().unwrap_or("."))?;

        let limit = input
            .max_results
            .unwrap_or(DEFAULT_RESULTS)
            .clamp(1, MAX_RESULTS);

        let hits = self
            .index
            .search(&root, &input.query, limit)
            .await
            .map_err(|error| {
                ToolError::Failed(format!("{error}. Use `search` or `read_symbol` instead."))
            })?;

        // Chunk paths are relative to the searched directory; show them
        // relative to the project so they can be passed to other tools.
        let prefix = context.workspace.relative(&root);

        Ok(render(&hits, &prefix))
    }
}

fn render(hits: &[Hit], prefix: &str) -> String {
    if hits.is_empty() {
        return "(nothing to search: no source files were found)".to_string();
    }

    let mut output = String::new();

    for (rank, hit) in hits.iter().enumerate() {
        output.push_str(&heading(hit, prefix));
        output.push('\n');

        if rank < RESULTS_WITH_SOURCE {
            output.push_str(&numbered_source(hit));
            output.push('\n');
        }
    }

    output
}

fn heading(hit: &Hit, prefix: &str) -> String {
    let chunk = &hit.chunk;

    let path = if prefix.is_empty() {
        chunk.path.clone()
    } else {
        format!("{prefix}/{}", chunk.path)
    };

    // The title is `path > symbol`; show only the symbol part after the
    // location, which already names the file.
    let name = chunk
        .title
        .split_once(" > ")
        .map(|(_, symbol)| symbol)
        .unwrap_or("");

    format!(
        "{path}:{}-{}  {name}  (similarity {:.2})",
        chunk.start_line, chunk.end_line, hit.score
    )
}

fn numbered_source(hit: &Hit) -> String {
    let chunk = &hit.chunk;

    let total = chunk.end_line - chunk.start_line + 1;

    let mut output = String::new();

    for (offset, line) in chunk.text.lines().take(MAX_SOURCE_LINES).enumerate() {
        output.push_str(&format!("{}\t{line}\n", chunk.start_line + offset));
    }

    if total > MAX_SOURCE_LINES {
        output.push_str(&format!(
            "(first {MAX_SOURCE_LINES} of {total} lines shown)\n"
        ));
    }

    output
}
