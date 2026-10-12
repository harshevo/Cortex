use std::sync::Arc;

use async_trait::async_trait;
use cortex_core::{Access, Tool, ToolContext, ToolError, ToolSpec};
use cortex_index::{CodeGraph, Dependency};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::input;

/// Shows what a file imports and which files import it.
pub struct FileDependencies {
    graph: Arc<CodeGraph>,
}

impl FileDependencies {
    pub fn new(graph: Arc<CodeGraph>) -> Self {
        Self { graph }
    }
}

#[derive(Deserialize)]
struct Input {
    path: String,
}

#[async_trait]
impl Tool for FileDependencies {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "file_dependencies".to_string(),

            description: "Show the import graph around one source file: the project files it \
                          imports, and every file that imports it, with the names taken. Use \
                          it to see what a change to a file can affect (including which tests \
                          cover it) and to find where a module is used. Imports of installed \
                          packages are listed by name only."
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

        let workspace = &context.workspace;
        let path = workspace.resolve(&input.path)?;

        if !path.is_file() {
            return Err(ToolError::InvalidInput(format!(
                "`{}` is not a file",
                input.path
            )));
        }

        let imports = self.graph.imports_of(workspace.root(), &path);
        let importers = self.graph.importers_of(workspace.root(), &path);

        let mut output = format!("{}\n", input.path);

        output.push_str(&format!("\nImports ({}):\n", imports.len()));

        for dependency in &imports {
            let target = match &dependency.target {
                Some(target) => workspace.relative(target),
                None => format!("{} (outside the project)", dependency.source),
            };

            output.push_str(&format!("  {target}{}\n", names(dependency)));
        }

        output.push_str(&format!("\nImported by ({}):\n", importers.len()));

        for dependency in &importers {
            output.push_str(&format!(
                "  {}:{}{}\n",
                workspace.relative(&dependency.importer),
                dependency.line,
                names(dependency),
            ));
        }

        Ok(output)
    }
}

fn names(dependency: &Dependency) -> String {
    let named: Vec<&str> = dependency
        .names
        .iter()
        .map(String::as_str)
        .filter(|name| *name != "*")
        .collect();

    if named.is_empty() {
        return String::new();
    }

    format!("  {{ {} }}", named.join(", "))
}
