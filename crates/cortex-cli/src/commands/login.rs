//! `cortex login`

use std::process::ExitCode;

use anyhow::Result;
use cortex_providers::codex;

pub async fn execute() -> Result<ExitCode> {
    codex::login(|address| {
        println!("Sign in to ChatGPT in your browser. If it did not open, go to:\n\n{address}\n");
    })
    .await?;

    println!("Signed in. Use the subscription with `--provider codex`.");

    Ok(ExitCode::SUCCESS)
}
