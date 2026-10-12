//! Gathering the context the router chose into one briefing.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cortex_core::{Briefing, ContextSource, Verifier, Workspace};
use cortex_index::{CodeGraph, walk};
use cortex_memory::MemorySystem;
use cortex_retrieval::SemanticIndex;
use cortex_tools::find_references;
use serde_json::{Value, json};

use crate::rules::{self, Available, Source};
use crate::symbols;

/// Names looked up in the graph for one task at most.
const MAX_SYMBOLS: usize = 3;

/// Uses listed per name. The agent can ask `find_references` for the rest.
const MAX_SITES_PER_SYMBOL: usize = 8;

/// Chunks asked for. Several can belong to one function, so fewer lines
/// than this are usually shown.
const SEMANTIC_HITS: usize = 8;

/// How long a task waits for the search by meaning before starting
/// without it.
const SEMANTIC_WAIT: Duration = Duration::from_secs(5);

/// The context sources of a run. Each is optional; the router uses the
/// ones that are present and that its rules pick for the task.
#[derive(Default)]
pub struct ContextRouter {
    memory: Option<Arc<MemorySystem>>,
    graph: Option<Arc<CodeGraph>>,
    semantic: Option<Arc<SemanticIndex>>,
    verifier: Option<Arc<dyn Verifier>>,
}

impl ContextRouter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_memory(mut self, memory: Arc<MemorySystem>) -> Self {
        self.memory = Some(memory);

        self
    }

    pub fn with_graph(mut self, graph: Arc<CodeGraph>) -> Self {
        self.graph = Some(graph);

        self
    }

    pub fn with_semantic(mut self, semantic: Arc<SemanticIndex>) -> Self {
        self.semantic = Some(semantic);

        self
    }

    pub fn with_verifier(mut self, verifier: Arc<dyn Verifier>) -> Self {
        self.verifier = Some(verifier);

        self
    }

    fn available(&self) -> Available {
        Available {
            memory: self.memory.is_some(),
            graph: self.graph.is_some(),
            semantic: self.semantic.is_some(),
            verifier: self.verifier.is_some(),
        }
    }

    /// The names in the task that are defined in the project.
    fn named_symbols(&self, instruction: &str, workspace: &Workspace) -> Vec<String> {
        let Some(graph) = &self.graph else {
            return Vec::new();
        };

        symbols::candidate_names(instruction)
            .into_iter()
            .filter(|name| {
                graph
                    .index()
                    .find(workspace.root(), |candidate| candidate == name)
                    .iter()
                    .any(|located| !walk::is_test_file(&workspace.relative(&located.path)))
            })
            .take(MAX_SYMBOLS)
            .collect()
    }

    async fn memory_section(&self, instruction: &str, workspace: &Workspace) -> Option<Section> {
        let briefing = self.memory.as_ref()?.brief(instruction, workspace).await?;

        Some(Section {
            text: briefing.text,
            detail: briefing.detail,
        })
    }

    fn symbols_section(&self, names: &[String], workspace: &Workspace) -> Option<Section> {
        let graph = self.graph.as_ref()?;

        let mut text = String::from(
            "## Code the task names\n\n\
             From the code graph. Call sites are resolved through imports.\n",
        );

        let mut counts = Vec::new();

        for name in names {
            let references = graph.references_to(workspace.root(), name);

            counts.push(json!({
                "name": name,
                "definitions": references.definitions.len(),
                "calls": references.calls().count(),
            }));

            text.push('\n');

            text.push_str(&find_references::render(
                name,
                &references,
                workspace,
                MAX_SITES_PER_SYMBOL,
            ));
        }

        Some(Section {
            text,
            detail: Value::Array(counts),
        })
    }

    async fn semantic_section(&self, instruction: &str, workspace: &Workspace) -> Option<Section> {
        let semantic = self.semantic.as_ref()?;

        // A search that cannot run is not worth failing the task over,
        // and one that would wait for the project to be embedded is not
        // worth holding the task up for: the model can still search.
        //
        // The search runs as its own task so that giving up on it does
        // not throw away what it has embedded so far: it carries on, and
        // a later task finds the index ready.
        let search = {
            let semantic = Arc::clone(semantic);
            let root = workspace.root().to_path_buf();
            let query = instruction.to_string();

            tokio::spawn(async move {
                semantic
                    .search_unless_busy(&root, &query, SEMANTIC_HITS)
                    .await
            })
        };

        let hits = tokio::time::timeout(SEMANTIC_WAIT, search)
            .await
            .ok()?
            .ok()?
            .ok()??;

        if hits.is_empty() {
            return None;
        }

        let mut text = String::from(
            "## Code that may be relevant\n\n\
             Found by comparing the task's wording with the code, best match first. These are \
             starting points, not a diagnosis: read before you change anything.\n\n",
        );

        let mut listed = Vec::new();
        let mut seen_titles = Vec::new();

        for hit in &hits {
            let chunk = &hit.chunk;

            // A long function is embedded as several windows; one line for
            // it is enough.
            if seen_titles.contains(&&chunk.title) {
                continue;
            }

            seen_titles.push(&chunk.title);

            text.push_str(&format!(
                "- {} (lines {}-{})\n",
                chunk.title, chunk.start_line, chunk.end_line
            ));

            listed.push(json!({
                "path": chunk.path,
                "start_line": chunk.start_line,
                "score": hit.score,
            }));
        }

        Some(Section {
            text,
            detail: Value::Array(listed),
        })
    }

    async fn checks_section(&self, workspace: &Workspace) -> Option<Section> {
        let verification = self.verifier.as_ref()?.verify(workspace).await;

        let failed: Vec<_> = verification.failures().collect();

        if failed.is_empty() {
            return None;
        }

        let mut text = String::from(
            "## The project's checks right now\n\n\
             The harness ran them before you started, so you do not need to run them again \
             to see what is failing.\n",
        );

        for check in &failed {
            text.push_str(&format!(
                "\n`{}` fails:\n\n{}\n",
                check.command, check.output
            ));
        }

        let commands: Vec<&str> = failed.iter().map(|check| check.command.as_str()).collect();

        Some(Section {
            text,
            detail: json!(commands),
        })
    }
}

/// One source's contribution to the briefing.
struct Section {
    text: String,
    detail: Value,
}

#[async_trait]
impl ContextSource for ContextRouter {
    async fn brief(&self, instruction: &str, workspace: &Workspace) -> Option<Briefing> {
        let kind = rules::classify(instruction);

        let names = self.named_symbols(instruction, workspace);

        let route = rules::route(kind, !names.is_empty(), self.available());

        let mut text = String::new();
        let mut used = serde_json::Map::new();

        for source in &route.sources {
            let section = match source {
                Source::Memory => self.memory_section(instruction, workspace).await,
                Source::Symbols => self.symbols_section(&names, workspace),
                Source::SemanticSearch => self.semantic_section(instruction, workspace).await,
                Source::CheckOutput => self.checks_section(workspace).await,
            };

            let Some(section) = section else {
                continue;
            };

            text.push_str(section.text.trim_end());
            text.push_str("\n\n");

            let key = serde_json::to_value(source)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_default();

            used.insert(key, section.detail);
        }

        // The decision is logged even when no source had anything to say,
        // because "nothing was shown" is also data for a learned router.
        Some(Briefing {
            source: "router".to_string(),
            text,
            detail: json!({
                "kind": route.kind,
                "chosen": route.sources,
                "symbols": names,
                "used": used,
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use cortex_index::CodeIndex;

    use super::*;

    fn project() -> (tempfile::TempDir, Workspace) {
        let dir = tempfile::tempdir().unwrap();

        fs::write(
            dir.path().join("money.ts"),
            "export function toCents(value: string): number {\n  return 1;\n}\n",
        )
        .unwrap();

        fs::write(
            dir.path().join("fees.ts"),
            "import { toCents } from \"./money\";\n\nexport function fee() {\n  return toCents(\"1\");\n}\n",
        )
        .unwrap();

        let workspace = Workspace::new(dir.path()).unwrap();

        (dir, workspace)
    }

    #[tokio::test]
    async fn a_named_function_arrives_with_its_call_sites() {
        let (_dir, workspace) = project();

        let graph = Arc::new(CodeGraph::new(Arc::new(CodeIndex::new())));
        let router = ContextRouter::new().with_graph(graph);

        let briefing = router
            .brief("`toCents` rounds the wrong way. Fix it.", &workspace)
            .await
            .unwrap();

        assert!(briefing.text.contains("money.ts:1-3"));
        assert!(briefing.text.contains("fees.ts:4  in fee"));

        assert_eq!(briefing.detail["kind"], "bug_fix");
        assert_eq!(briefing.detail["symbols"][0], "toCents");
        assert_eq!(briefing.detail["used"]["symbols"][0]["calls"], 1);
    }

    #[tokio::test]
    async fn a_task_with_no_usable_source_still_logs_its_route() {
        let (_dir, workspace) = project();

        let briefing = ContextRouter::new()
            .brief("Evening trades are filed under the wrong day.", &workspace)
            .await
            .unwrap();

        assert!(briefing.text.is_empty());
        assert_eq!(briefing.detail["kind"], "bug_fix");
    }
}
