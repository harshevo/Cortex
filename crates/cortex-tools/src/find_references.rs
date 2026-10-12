use std::sync::Arc;

use async_trait::async_trait;
use cortex_core::{Access, Tool, ToolContext, ToolError, ToolSpec, Workspace};
use cortex_index::{CodeGraph, References, Site, UseKind};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::input;

const DEFAULT_RESULTS: usize = 60;
const MAX_RESULTS: usize = 200;

/// Lists every place a function, class or type is used, following imports.
pub struct FindReferences {
    graph: Arc<CodeGraph>,
}

impl FindReferences {
    pub fn new(graph: Arc<CodeGraph>) -> Self {
        Self { graph }
    }
}

#[derive(Deserialize)]
struct Input {
    name: String,

    #[serde(default)]
    max_results: Option<usize>,
}

#[async_trait]
impl Tool for FindReferences {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "find_references".to_string(),

            description: "Find every place a function, class, type or constant is used: its \
                          definition, each call site with the line of code and the function it \
                          is in, and the files that import it. Follows imports, aliases and \
                          re-exports, so it does not return unrelated code that merely shares \
                          the name. Use it before changing a signature or renaming something, \
                          and to see how a function is used. Method calls on values of unknown \
                          type are listed separately as possible matches. Supports Python, \
                          TypeScript, JavaScript and Rust."
                .to_string(),

            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "The exact name, for example `matchFills`.",
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum uses to list (default 60).",
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

        let limit = input
            .max_results
            .unwrap_or(DEFAULT_RESULTS)
            .clamp(1, MAX_RESULTS);

        let name = input.name.trim();

        let references = self.graph.references_to(context.workspace.root(), name);

        if references.definitions.is_empty() && references.sites.is_empty() {
            return Ok(format!(
                "Nothing named `{name}` is defined or used in the project's source files. \
                 The name must match exactly; use `search` for partial names."
            ));
        }

        Ok(render(name, &references, &context.workspace, limit))
    }
}

/// The report shared by this tool and the router's briefing.
pub fn render(name: &str, references: &References, workspace: &Workspace, limit: usize) -> String {
    let mut output = String::new();

    if references.definitions.is_empty() {
        output.push_str(&format!(
            "`{name}` is not defined in the project (it may come from a package).\n"
        ));
    } else {
        output.push_str(&format!("`{name}` is defined at:\n"));

        for located in &references.definitions {
            let symbol = &located.symbol;

            output.push_str(&format!(
                "  {}:{}-{}  {}\n",
                workspace.relative(&located.path),
                symbol.start_line,
                symbol.end_line,
                symbol.signature,
            ));
        }
    }

    let mut budget = limit;

    let calls: Vec<&Site> = references.calls().collect();
    let mentions: Vec<&Site> = references.resolved(UseKind::Mention).collect();
    let imports: Vec<&Site> = references.resolved(UseKind::Import).collect();
    let possible: Vec<&Site> = references.possible().collect();

    section(&mut output, "Call sites", &calls, workspace, &mut budget);

    section(
        &mut output,
        "Other uses (passed as a value, used as a type)",
        &mentions,
        workspace,
        &mut budget,
    );

    section(&mut output, "Imported in", &imports, workspace, &mut budget);

    section(
        &mut output,
        "Possible matches (same name, not tied to the definition by an import)",
        &possible,
        workspace,
        &mut budget,
    );

    if calls.is_empty() && mentions.is_empty() && possible.is_empty() {
        output.push_str("\nNo uses found outside its definition.\n");
    }

    output
}

fn section(
    output: &mut String,
    title: &str,
    sites: &[&Site],
    workspace: &Workspace,
    budget: &mut usize,
) {
    if sites.is_empty() {
        return;
    }

    output.push_str(&format!("\n{title} ({}):\n", sites.len()));

    let shown = sites.len().min(*budget);

    for site in &sites[..shown] {
        let location = format!("{}:{}", workspace.relative(&site.path), site.line);

        let place = match &site.enclosing {
            Some(enclosing) => format!("  in {enclosing}"),
            None => String::new(),
        };

        output.push_str(&format!("  {location}{place}\n      {}\n", site.text));
    }

    *budget -= shown;

    if shown < sites.len() {
        output.push_str(&format!(
            "  ... {} more not shown; raise `max_results` to see them\n",
            sites.len() - shown
        ));
    }
}
