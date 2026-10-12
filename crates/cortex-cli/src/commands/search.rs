//! `cortex search "<description>"`
//!
//! Runs semantic search directly, without a model. Useful for trying the
//! index on a project and for measuring retrieval quality on its own.

use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, Result};
use cortex_core::Workspace;
use cortex_index::CodeIndex;
use cortex_retrieval::{EmbeddingClient, SemanticIndex};

use crate::args::SearchArgs;

pub async fn execute(args: SearchArgs) -> Result<ExitCode> {
    let workspace = Workspace::new(&args.dir)
        .with_context(|| format!("cannot open the project directory {}", args.dir.display()))?;

    let client = EmbeddingClient::new(&args.embedding_url);

    client.health().await?;

    let index = SemanticIndex::new(client, Arc::new(CodeIndex::new()));

    let hits = index
        .search(workspace.root(), &args.query, args.max_results)
        .await?;

    for hit in hits {
        let chunk = &hit.chunk;

        println!(
            "{:.3}  {}:{}-{}  {}",
            hit.score, chunk.path, chunk.start_line, chunk.end_line, chunk.title
        );
    }

    Ok(ExitCode::SUCCESS)
}
