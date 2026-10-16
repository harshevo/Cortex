//! `cortex prepare`
//!
//! Work that can be done before any task is given, so that the task does
//! not have to wait for it or pay for it step by step: parsing the code
//! into the graph, embedding it for search by meaning, and having the
//! model read a map of the project once and write down what a newcomer
//! should know. The overview is returned to the caller.
//!
//! This is the idea of sleep-time compute (Lin et al., 2025): think about
//! the context ahead of the question, when the question is predictable.
//! "What is this project and where is what" is asked by every task.

use std::fs;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use cortex_core::{ModelProvider, Usage, Workspace, judge};
use cortex_index::graph::GraphStats;
use cortex_index::{CodeGraph, CodeIndex, repo_map};
use cortex_retrieval::{EmbeddingClient, IndexSize};

use crate::args::{ModelArgs, PrepareArgs};
use crate::session;

/// How much of the project the model is shown.
const MAX_MAP_CHARS: usize = 14_000;
const MAX_README_CHARS: usize = 4_000;
const MAX_MANIFEST_CHARS: usize = 2_000;

/// Files that say how a project is built and tested.
const MANIFESTS: [&str; 5] = [
    "package.json",
    "Cargo.toml",
    "pyproject.toml",
    "Makefile",
    "go.mod",
];

const SYSTEM: &str = "\
You are shown a map of a software project (its source files, with the names \
each defines) and parts of its own documentation and build files. Write what \
a programmer about to make a change here should know before starting.

Write at most eight short bullet points: what the project is, which \
directory holds what, where the tests are and which command runs them, and \
any convention the names make plain. Say only what the material shows; if it \
does not show how to run the tests, leave that out. No introduction and no \
closing remark.";

/// What was got ready.
pub struct Prepared {
    pub graph: GraphStats,
    pub graph_ms: u64,

    /// `None` when the embedding service is not running.
    pub embedded: Option<IndexSize>,
    pub embedded_ms: u64,

    /// The description the model wrote, when a model was used.
    pub overview: Option<String>,
    pub usage: Usage,
}

pub async fn execute(args: PrepareArgs) -> Result<ExitCode> {
    let workspace = Workspace::new(&args.dir)
        .with_context(|| format!("cannot open the project directory {}", args.dir.display()))?;

    let provider = if args.no_model {
        None
    } else {
        Some(session::build_provider(&args.model)?)
    };

    let prepared = prepare(&workspace, &args.model, provider).await?;

    println!(
        "code graph: {} source files, {} imports inside the project, {} calls ({} ms)",
        prepared.graph.files,
        prepared.graph.resolved_imports,
        prepared.graph.calls,
        prepared.graph_ms
    );

    match &prepared.embedded {
        Some(size) => println!(
            "embeddings: {} chunks of {} files, {} KB held ({} ms)",
            size.chunks,
            size.files,
            size.bytes / 1_000,
            prepared.embedded_ms
        ),

        None => println!(
            "embeddings: skipped, the embedding service is not running (start it with \
             `uv run --project ml python -m cortex_ml.embedding.server`)"
        ),
    }

    match &prepared.overview {
        Some(overview) => println!(
            "\nproject description ({} tokens in, {} out):\n\n{overview}",
            prepared.usage.total_input(),
            prepared.usage.output_tokens
        ),

        None => println!("project description: not written (no model was used)"),
    }

    Ok(ExitCode::SUCCESS)
}

/// Gets the project in `workspace` ready. The description is written only
/// when a `provider` is given.
pub async fn prepare(
    workspace: &Workspace,
    args: &ModelArgs,
    provider: Option<Arc<dyn ModelProvider>>,
) -> Result<Prepared> {
    let root = workspace.root();

    let index = Arc::new(CodeIndex::new());

    let started = Instant::now();

    let graph = CodeGraph::new(index.clone()).stats(root);

    let graph_ms = started.elapsed().as_millis() as u64;

    let started = Instant::now();

    let client = EmbeddingClient::new(&args.embedding_url);

    // The service keeps every vector by the text it came from, so what is
    // embedded here is not embedded again when a run asks for it.
    let embedded = match client.health().await {
        Ok(_) => Some(cortex_tools::semantic_index(client).warm(root).await?),
        Err(_) => None,
    };

    let embedded_ms = started.elapsed().as_millis() as u64;

    let mut prepared = Prepared {
        graph,
        graph_ms,
        embedded,
        embedded_ms,
        overview: None,
        usage: Usage::default(),
    };

    let Some(provider) = provider else {
        return Ok(prepared);
    };

    let Some(material) = material(&index, workspace) else {
        return Ok(prepared);
    };

    let answer = judge::ask(provider.as_ref(), SYSTEM, &material).await?;

    let overview = answer.text.trim().to_string();

    if overview.is_empty() {
        return Ok(prepared);
    }

    prepared.overview = Some(overview);
    prepared.usage = answer.usage;

    Ok(prepared)
}

/// What the model is shown of the project. `None` when it has no source
/// file the index understands.
fn material(index: &CodeIndex, workspace: &Workspace) -> Option<String> {
    let root = workspace.root();

    let map = repo_map(index, root, MAX_MAP_CHARS)?;

    let mut text = format!("## Project map\n\n{}\n", map.text);

    let head = |name: &str, max_chars: usize| -> Option<String> {
        let content = fs::read_to_string(root.join(name)).ok()?;

        Some(content.chars().take(max_chars).collect())
    };

    for readme in ["README.md", "README", "readme.md"] {
        if let Some(content) = head(readme, MAX_README_CHARS) {
            text.push_str(&format!("\n## {readme} (start)\n\n{content}\n"));

            break;
        }
    }

    for manifest in MANIFESTS {
        if let Some(content) = head(manifest, MAX_MANIFEST_CHARS) {
            text.push_str(&format!("\n## {manifest} (start)\n\n{content}\n"));
        }
    }

    Some(text)
}

#[cfg(test)]
mod tests {
    use cortex_providers::ScriptedProvider;
    use cortex_providers::scripted::final_turn;

    use super::*;

    #[tokio::test]
    async fn the_project_overview_is_returned_without_persistent_memory() {
        let dir = tempfile::tempdir().unwrap();

        fs::write(
            dir.path().join("fees.py"),
            "def compute_fee(amount):\n    return amount\n",
        )
        .unwrap();

        fs::write(dir.path().join("README.md"), "# Fees\n\nRun `pytest`.\n").unwrap();

        let workspace = Workspace::new(dir.path()).unwrap();

        let args = ModelArgs {
            // Nothing listens here, so embedding is skipped.
            embedding_url: "http://127.0.0.1:9".to_string(),
            ..model_args()
        };

        let provider = Arc::new(ScriptedProvider::new(vec![final_turn(
            "- A fee calculator; `fees.py` holds `compute_fee`.\n- Tests run with `pytest`.",
        )]));

        let prepared = prepare(&workspace, &args, Some(provider)).await.unwrap();

        assert_eq!(prepared.graph.files, 1);
        assert!(prepared.embedded.is_none());
        assert!(
            prepared
                .overview
                .as_deref()
                .unwrap()
                .contains("fee calculator")
        );
    }

    fn model_args() -> ModelArgs {
        use clap::Parser;

        #[derive(Parser)]
        struct Only {
            #[command(flatten)]
            model: ModelArgs,
        }

        Only::parse_from(["cortex"]).model
    }
}
