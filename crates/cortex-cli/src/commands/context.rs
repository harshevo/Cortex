//! `cortex context "<instruction>"`
//!
//! Shows, without calling a model, what the switched-on capabilities would
//! put in front of an instruction and how long gathering it takes. Useful
//! for seeing what memory and the router contribute, and for measuring
//! their overhead on a project.

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use cortex_core::Workspace;
use cortex_index::{CodeGraph, CodeIndex};

use crate::args::ContextArgs;
use crate::features::{self, MemoryLocation};

pub async fn execute(args: ContextArgs) -> Result<ExitCode> {
    let workspace = Workspace::new(&args.dir)
        .with_context(|| format!("cannot open the project directory {}", args.dir.display()))?;

    let memory_location = MemoryLocation {
        path: args
            .model
            .memory_db
            .clone()
            .unwrap_or_else(|| workspace.root().join(".cortex").join("memory.db")),
        scope: args
            .scope
            .clone()
            .unwrap_or_else(|| workspace.root().display().to_string()),
        skip_identical: false,
    };

    let setup = features::setup(&args.model, memory_location).await?;

    print_graph_size(&workspace);

    for source in &setup.context_sources {
        let started = Instant::now();

        let briefing = source.brief(&args.instruction, &workspace).await;

        let elapsed = started.elapsed();

        match briefing {
            Some(briefing) => {
                println!(
                    "--- {} ({} characters, {} ms) ---\n",
                    briefing.source,
                    briefing.text.chars().count(),
                    elapsed.as_millis()
                );

                println!("{}", briefing.text.trim_end());
                println!("\nHow it was chosen: {}\n", briefing.detail);
            }

            None => println!("--- nothing to show ({} ms) ---\n", elapsed.as_millis()),
        }
    }

    if setup.context_sources.is_empty() {
        println!("No context source is on. Try `--with memory` or `--with router`.\n");
    }

    let Some(verifier) = &setup.verifier else {
        return Ok(ExitCode::SUCCESS);
    };

    println!("--- verification gate ---\n");

    for command in verifier.commands(&workspace) {
        println!("would run: {command}");
    }

    if args.run_checks {
        let verification = verifier.verify(&workspace).await;

        for check in &verification.checks {
            let verdict = if check.passed { "passed" } else { "FAILED" };

            println!(
                "\n`{}` {verdict} in {} ms",
                check.command, check.duration_ms
            );

            if !check.passed {
                println!("\n{}", check.output);
            }
        }
    }

    Ok(ExitCode::SUCCESS)
}

/// How much the code graph covers, and how long a cold build takes.
fn print_graph_size(workspace: &Workspace) {
    let graph = CodeGraph::new(Arc::new(CodeIndex::new()));

    let started = Instant::now();

    let stats = graph.stats(workspace.root());

    println!(
        "code graph: {} source files, {} imports ({} inside the project), {} calls; built in {} ms\n",
        stats.files,
        stats.imports,
        stats.resolved_imports,
        stats.calls,
        started.elapsed().as_millis()
    );
}
