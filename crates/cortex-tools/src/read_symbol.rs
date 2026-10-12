use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use cortex_core::{Access, Tool, ToolContext, ToolError, ToolSpec, Workspace};
use cortex_index::CodeIndex;
use cortex_index::index::Located;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::input;

/// Above these limits the tool lists where the matches are instead of
/// printing them, so one broad name cannot flood the context.
const MAX_MATCHES_SHOWN_IN_FULL: usize = 4;
const MAX_LINES_SHOWN_IN_FULL: usize = 400;
const MAX_LOCATIONS_LISTED: usize = 50;

/// Finds a definition by name and returns its source.
pub struct ReadSymbol {
    index: Arc<CodeIndex>,
}

impl ReadSymbol {
    pub fn new(index: Arc<CodeIndex>) -> Self {
        Self { index }
    }
}

#[derive(Deserialize)]
struct Input {
    name: String,

    #[serde(default)]
    path: Option<String>,
}

#[async_trait]
impl Tool for ReadSymbol {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read_symbol".to_string(),

            description: "Find where a function, class, method or type is defined and return \
                          its source with line numbers, in one step. Searches the whole project \
                          unless `path` narrows it to a file or directory. Prefer this to \
                          `search` followed by `read_file` when you know the name of what you \
                          are looking for. If the name is defined in many places, the locations \
                          are listed instead."
                .to_string(),

            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "The symbol's name, for example `matchFills` or `Cart`.",
                    },
                    "path": {
                        "type": "string",
                        "description": "File or directory to look in. Defaults to the project.",
                    },
                },
                "required": ["name"],
            }),
        }
    }

    fn access(&self) -> Access {
        Access::ReadOnly
    }

    async fn run(&self, input: Value, context: &ToolContext) -> Result<String, ToolError> {
        let input: Input = input::parse(input)?;

        let root = context
            .workspace
            .resolve(input.path.as_deref().unwrap_or("."))?;

        let index = self.index.clone();
        let workspace = context.workspace.clone();

        let output = tokio::task::spawn_blocking(move || {
            let matches = index.find_by_name(&root, &input.name);

            render(&workspace, &input.name, &matches)
        })
        .await
        .map_err(|error| ToolError::Failed(error.to_string()))?;

        Ok(output)
    }
}

fn render(workspace: &Workspace, name: &str, matches: &[Located]) -> String {
    if matches.is_empty() {
        return format!(
            "No definition named `{name}` was found. It may be in a language the index does \
             not support; try `search`."
        );
    }

    let total_lines: usize = matches
        .iter()
        .map(|located| located.symbol.end_line - located.symbol.start_line + 1)
        .sum();

    let fits = matches.len() <= MAX_MATCHES_SHOWN_IN_FULL && total_lines <= MAX_LINES_SHOWN_IN_FULL;

    if !fits {
        return render_locations(workspace, name, matches);
    }

    matches
        .iter()
        .map(|located| render_source(workspace, located))
        .collect::<Vec<_>>()
        .join("\n")
}

fn heading(workspace: &Workspace, located: &Located) -> String {
    let symbol = &located.symbol;

    let owner = match &symbol.container {
        Some(container) => format!(" in {container}"),
        None => String::new(),
    };

    format!(
        "{}:{}-{}  {} {}{owner}",
        workspace.relative(&located.path),
        symbol.start_line,
        symbol.end_line,
        symbol.kind.label(),
        symbol.name,
    )
}

fn render_source(workspace: &Workspace, located: &Located) -> String {
    let mut output = heading(workspace, located);

    output.push('\n');
    output.push_str(&numbered_lines(
        &located.path,
        located.symbol.start_line,
        located.symbol.end_line,
    ));

    output
}

fn render_locations(workspace: &Workspace, name: &str, matches: &[Located]) -> String {
    let mut output = format!(
        "`{name}` matches {} definitions. Read the one you need with `read_file` and its line \
         range, or call `read_symbol` again with `path`.\n",
        matches.len()
    );

    for located in matches.iter().take(MAX_LOCATIONS_LISTED) {
        output.push_str(&format!("  {}\n", heading(workspace, located)));
    }

    if matches.len() > MAX_LOCATIONS_LISTED {
        output.push_str(&format!(
            "  ... and {} more\n",
            matches.len() - MAX_LOCATIONS_LISTED
        ));
    }

    output
}

/// Lines `first..=last` of a file, each prefixed with its number, in the
/// same format `read_file` uses.
fn numbered_lines(path: &Path, first: usize, last: usize) -> String {
    let Ok(source) = std::fs::read_to_string(path) else {
        return "(the file could not be read)\n".to_string();
    };

    let mut output = String::new();

    for (index, line) in source
        .lines()
        .enumerate()
        .skip(first - 1)
        .take(last - first + 1)
    {
        output.push_str(&format!("{}\t{line}\n", index + 1));
    }

    output
}
