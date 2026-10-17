use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use cortex_core::learn;
use cortex_core::notes::Notes;
use cortex_core::{Access, Tool, ToolContext, ToolError, ToolSpec};
use cortex_index::CodeGraph;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::input;

/// Files beyond this many are counted, not listed.
const MAX_LISTED: usize = 15;

/// How far dependents are followed.
const DEPTH: usize = 2;

/// What a change to some files or a name would reach: the code that
/// depends on it, the tests that cover it, its recent history, what was
/// noted about it and how earlier changes to it went.
pub struct Impact {
    graph: Arc<CodeGraph>,
}

impl Impact {
    pub fn new(graph: Arc<CodeGraph>) -> Self {
        Self { graph }
    }
}

#[derive(Deserialize)]
struct Input {
    #[serde(default)]
    paths: Vec<String>,

    #[serde(default)]
    symbol: Option<String>,
}

#[async_trait]
impl Tool for Impact {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "impact".to_string(),

            description: "Before changing code, see what the change would reach: for files, the \
                          files that import them (two levels deep), the tests that cover them, \
                          their last commits, project notes about them, and how earlier runs \
                          that changed them went; for a symbol, where it is defined and every \
                          place that calls or uses it. Use it to plan a change that touches \
                          shared code, and to decide which tests to run."
                .to_string(),

            input_schema: json!({
                "type": "object",
                "properties": {
                    "paths": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Files that will change, relative to the project.",
                    },
                    "symbol": {
                        "type": "string",
                        "description": "A function, class or other name that will change.",
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
        let workspace = &context.workspace;
        let root = workspace.root();

        if input.paths.is_empty() && input.symbol.is_none() {
            return Err(ToolError::InvalidInput(
                "give `paths`, a `symbol`, or both".to_string(),
            ));
        }

        let mut output = String::new();
        let mut files: Vec<PathBuf> = input
            .paths
            .iter()
            .map(|path| workspace.resolve(path))
            .collect::<Result<_, _>>()?;

        if let Some(symbol) = &input.symbol {
            let references = self.graph.references_to(root, symbol);

            output.push_str(&format!("Symbol `{symbol}`\n"));

            for definition in &references.definitions {
                output.push_str(&format!(
                    "  defined in {}:{}\n",
                    workspace.relative(&definition.path),
                    definition.symbol.start_line
                ));

                files.push(definition.path.clone());
            }

            let mut uses: BTreeMap<String, Vec<String>> = BTreeMap::new();

            for site in &references.sites {
                let place = match &site.enclosing {
                    Some(enclosing) => format!("{} (in {enclosing})", site.line),
                    None => site.line.to_string(),
                };

                uses.entry(workspace.relative(&site.path))
                    .or_default()
                    .push(place);
            }

            output.push_str(&format!(
                "  used in {} files ({} places)\n",
                uses.len(),
                references.sites.len()
            ));

            for (file, places) in uses.iter().take(MAX_LISTED) {
                output.push_str(&format!("    {file}: lines {}\n", places.join(", ")));
            }

            output.push('\n');
        }

        files.sort();
        files.dedup();

        if files.is_empty() {
            return Ok(output);
        }

        // Dependents, level by level.
        let mut seen: BTreeSet<PathBuf> = files.iter().cloned().collect();
        let mut level = files.clone();

        output.push_str("Depends on these files\n");

        for depth in 1..=DEPTH {
            let mut next = BTreeSet::new();

            for file in &level {
                for dependency in self.graph.importers_of(root, file) {
                    if seen.insert(dependency.importer.clone()) {
                        next.insert(dependency.importer);
                    }
                }
            }

            let label = if depth == 1 { "directly" } else { "through those" };

            output.push_str(&format!("  {label}: {}\n", next.len()));

            for file in next.iter().take(MAX_LISTED) {
                output.push_str(&format!("    {}\n", workspace.relative(file)));
            }

            level = next.into_iter().collect();
        }

        let tests = self.graph.tests_affected_by(root, &files);

        output.push_str(&format!("\nTests that reach them: {}\n", tests.len()));

        for test in tests.iter().take(MAX_LISTED) {
            output.push_str(&format!("  {}\n", workspace.relative(test)));
        }

        let relative: Vec<String> = files.iter().map(|file| workspace.relative(file)).collect();

        let history = recent_commits(root, &relative).await;

        if !history.is_empty() {
            output.push_str("\nRecent commits\n");
            output.push_str(&history);
        }

        let notes = Notes::for_project(root);

        let noted: Vec<String> = notes
            .all()
            .into_iter()
            .filter(|note| note.files.keys().any(|file| relative.contains(file)))
            .map(|note| format!("  #{} [{}] {}", note.id, note.kind.label(), note.text))
            .collect();

        if !noted.is_empty() {
            output.push_str("\nNotes about them\n");
            output.push_str(&noted.join("\n"));
            output.push('\n');
        }

        let earlier: Vec<String> = notes
            .episodes()
            .into_iter()
            .rev()
            .filter(|episode| episode.changed.iter().any(|file| relative.contains(file)))
            .take(5)
            .map(|episode| {
                let ending = if learn::passed(&episode) { "passed" } else { "did not pass" };

                format!("  \"{}\": {ending}", episode.instruction)
            })
            .collect();

        if !earlier.is_empty() {
            output.push_str("\nEarlier runs that changed them\n");
            output.push_str(&earlier.join("\n"));
            output.push('\n');
        }

        Ok(output)
    }
}

/// The last few commits that touched `files`, one per line.
async fn recent_commits(root: &std::path::Path, files: &[String]) -> String {
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["log", "-n", "5", "--format=  %h %s (%ar)", "--"])
        .args(files)
        .output()
        .await;

    match output {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout).into(),
        _ => String::new(),
    }
}
