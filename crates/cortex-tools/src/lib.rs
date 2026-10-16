//! The built-in tools.
//!
//! One file per tool. Each tool declares its JSON input schema, whether it
//! can change anything, and how to run.

mod affected_tests;
mod browser;
mod calculate;
mod checks;
mod edit_file;
mod file_dependencies;
pub mod find_references;
mod github;
mod input;
mod list_files;
mod outline;
mod read_file;
mod read_symbol;
mod search;
mod semantic_search;
mod shell;
mod write_file;

use std::sync::Arc;

use cortex_core::ToolRegistry;
use cortex_index::{CodeGraph, CodeIndex};
use cortex_retrieval::{EmbeddingClient, SemanticIndex};

pub use affected_tests::AffectedTests;
pub use browser::{Browser, BrowserOptions, BrowserView};
pub use edit_file::EditFile;
pub use file_dependencies::FileDependencies;
pub use find_references::FindReferences;
pub use github::{Github, GithubWrite};
pub use list_files::ListFiles;
pub use outline::Outline;
pub use read_file::ReadFile;
pub use read_symbol::ReadSymbol;
pub use search::Search;
pub use semantic_search::SemanticSearch;
pub use shell::Shell;
pub use write_file::WriteFile;

/// The basic tools every configuration has.
pub fn default_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();

    registry.register(ReadFile::default());
    registry.register(ListFiles);
    registry.register(Search);
    registry.register(WriteFile);
    registry.register(EditFile::default());
    registry.register(Shell::default());
    registry.register(calculate::Calculate);

    registry
}

/// Swaps three basic tools for versions that return less text:
///
/// - `read_file` answers with an outline when asked for a whole long file.
/// - `edit_file` shows the changed lines, which saves a read to confirm.
/// - `shell` keeps only the summary of a passing test, build or lint run,
///   and cleans up a failing one.
pub fn add_lean_tools(registry: &mut ToolRegistry) {
    registry.register(ReadFile::lean(Arc::new(CodeIndex::new())));
    registry.register(EditFile::lean());
    registry.register(Shell::lean());
}

/// Adds the tools that answer questions from the code index: what a file
/// contains, and where a name is defined.
pub fn add_code_index_tools(registry: &mut ToolRegistry) {
    let index = Arc::new(CodeIndex::new());

    registry.register(Outline::new(index.clone()));
    registry.register(ReadSymbol::new(index));
}

/// Adds the tools that answer questions from the code graph: where a name
/// is used, which files depend on which, and which tests a change affects.
pub fn add_code_graph_tools(registry: &mut ToolRegistry, graph: Arc<CodeGraph>) {
    registry.register(FindReferences::new(graph.clone()));
    registry.register(AffectedTests::new(graph.clone()));
    registry.register(FileDependencies::new(graph));
}

/// Adds `github`, which reads pull requests, issues and workflow runs, and
/// `github_write`, which posts and so asks for approval each time. Both
/// need the `gh` command, signed in.
pub fn add_github_tools(registry: &mut ToolRegistry) {
    registry.register(Github::default());
    registry.register(GithubWrite::default());
}

/// Adds `browser`, which drives a real browser and so asks for approval
/// each time, and `browser_view`, which only looks at the page it has
/// open. Both need a browser built on Chromium to be installed.
pub fn add_browser_tools(registry: &mut ToolRegistry, options: BrowserOptions) {
    let (browser, view) = browser::tools(options);

    registry.register(browser);
    registry.register(view);
}

/// Adds `semantic_search`, which needs the embedding service to be running.
pub fn add_semantic_search(registry: &mut ToolRegistry, index: Arc<SemanticIndex>) {
    registry.register(SemanticSearch::new(index));
}

/// A semantic index over its own copy of the code index.
pub fn semantic_index(client: EmbeddingClient) -> Arc<SemanticIndex> {
    Arc::new(SemanticIndex::new(client, Arc::new(CodeIndex::new())))
}
