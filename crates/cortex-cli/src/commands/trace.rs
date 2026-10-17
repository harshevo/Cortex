//! `cortex trace`: what a run did, how two runs differ, and whether a
//! run's tool calls still give the same output.

use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use cortex_core::rollout::copy_project;
use cortex_core::trajectory::Event;
use cortex_core::{AutoApprove, Checkpoints, Sandbox, ToolContext, trace};

use crate::args::TraceArgs;

pub async fn execute(args: TraceArgs) -> Result<ExitCode> {
    let events = trace::read(&args.log)
        .with_context(|| format!("could not read {}", args.log.display()))?;

    let run = pick(&events, args.run)?;

    if let Some(other) = &args.compare {
        let other_events = trace::read(other)
            .with_context(|| format!("could not read {}", other.display()))?;

        let other_run = pick(&other_events, args.other_run)?;

        println!("{}", trace::compare(run, other_run));

        return Ok(ExitCode::SUCCESS);
    }

    println!("{}", trace::summarize(run).render());

    if args.steps {
        println!();

        for action in trace::actions(run) {
            println!("{}", action.describe());
        }
    }

    if args.replay {
        println!("\n{}", replay(run, &args).await?);
    }

    Ok(ExitCode::SUCCESS)
}

/// The run numbered `number` from 1, or the last.
fn pick(events: &[Event], number: Option<usize>) -> Result<&[Event]> {
    let runs = trace::runs(events);

    if runs.is_empty() {
        bail!("the log holds no run");
    }

    match number {
        None => Ok(runs[runs.len() - 1]),
        Some(number) if (1..=runs.len()).contains(&number) => Ok(runs[number - 1]),
        Some(number) => bail!("there is no run {number}; the log holds {}", runs.len()),
    }
}

/// Makes the run's calls again in a copy of the project, put back as it
/// was when the run started where its checkpoint allows.
async fn replay(run: &[Event], args: &TraceArgs) -> Result<String> {
    let root = args.dir.canonicalize()?;

    let sandbox = if Sandbox::available() {
        Sandbox::On
    } else {
        Sandbox::Off
    };

    let (_copy, workspace) = copy_project(&root, sandbox).await?;

    let checkpoint = run.iter().find_map(|event| match event {
        Event::CheckpointSaved { id, .. } if !id.is_empty() => Some(id.clone()),
        _ => None,
    });

    let mut note = "Replayed in a copy of the project as it is now.".to_string();

    if let Some(id) = checkpoint
        && Checkpoints::for_project(workspace.root())
            .restore(&id)
            .await
            .is_ok()
    {
        note = format!("Replayed in a copy put back to checkpoint {}.", &id[..id.len().min(8)]);
    }

    let context = ToolContext {
        workspace,
        approver: Arc::new(AutoApprove),
    };

    let tools = cortex_tools::default_registry();
    let replayed = trace::replay_tools(run, &tools, &context).await;

    Ok(format!("{note}\n{}", trace::render_replay(&replayed)))
}
