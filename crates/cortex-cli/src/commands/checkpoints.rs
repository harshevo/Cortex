//! `cortex undo` and `cortex checkpoints`.

use std::process::ExitCode;

use anyhow::{Result, anyhow};
use cortex_core::{Checkpoints, Restored};

use crate::args::CheckpointArgs;

/// Lists the checkpoints, or restores the one named.
pub async fn execute(args: CheckpointArgs) -> Result<ExitCode> {
    let checkpoints = Checkpoints::for_project(args.dir.canonicalize()?);

    if let Some(id) = &args.id {
        return restore(&checkpoints, id).await;
    }

    let list = checkpoints.list(30).await.map_err(|error| anyhow!(error))?;

    if list.is_empty() {
        println!("No checkpoints yet. One is saved before a run first changes the project.");

        return Ok(ExitCode::SUCCESS);
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default();

    for checkpoint in list {
        println!(
            "{}  {:>8}  {}",
            checkpoint.short(),
            ago(now.saturating_sub(checkpoint.time)),
            checkpoint.label
        );
    }

    Ok(ExitCode::SUCCESS)
}

/// Restores the checkpoint named, or the latest one saved before a run.
pub async fn undo(args: CheckpointArgs) -> Result<ExitCode> {
    let checkpoints = Checkpoints::for_project(args.dir.canonicalize()?);

    let id = match args.id {
        Some(id) => id,
        None => latest_before_a_run(&checkpoints).await?,
    };

    restore(&checkpoints, &id).await
}

/// The newest checkpoint a run saved, passing over those a restore saved.
pub async fn latest_before_a_run(checkpoints: &Checkpoints) -> Result<String> {
    checkpoints
        .list(50)
        .await
        .map_err(|error| anyhow!(error))?
        .into_iter()
        .find(|checkpoint| checkpoint.label != "before restoring a checkpoint")
        .map(|checkpoint| checkpoint.id)
        .ok_or_else(|| anyhow!("there is no checkpoint to go back to"))
}

async fn restore(checkpoints: &Checkpoints, id: &str) -> Result<ExitCode> {
    let restored = checkpoints
        .restore(id)
        .await
        .map_err(|error| anyhow!(error))?;

    println!("{}", describe(&restored));

    Ok(ExitCode::SUCCESS)
}

/// What a restore did, in a few lines.
pub fn describe(restored: &Restored) -> String {
    let short = &restored.to[..restored.to.len().min(8)];

    if restored.changed() == 0 {
        return format!("Already at checkpoint {short}; nothing changed.");
    }

    let mut text = format!(
        "Restored checkpoint {short}: {} file(s) put back, {} removed.",
        restored.reverted.len(),
        restored.removed.len()
    );

    if let Some(before) = &restored.before {
        text.push_str(&format!(
            "\nThe state before this is checkpoint {}, if you want it back.",
            before.short()
        ));
    }

    text
}

fn ago(seconds: u64) -> String {
    match seconds {
        0..60 => format!("{seconds}s ago"),
        60..3600 => format!("{}m ago", seconds / 60),
        3600..86400 => format!("{}h ago", seconds / 3600),
        _ => format!("{}d ago", seconds / 86400),
    }
}
