//! `cortex chat`

use std::io::{self, Write};
use std::process::ExitCode;

use anyhow::Result;

use crate::args::ChatArgs;
use crate::commands::skills;
use crate::session;

pub async fn execute(args: ChatArgs) -> Result<ExitCode> {
    let agent = session::build_agent(&args.session).await?;

    let mut conversation = Vec::new();

    println!("Cortex. Type an instruction, `/skills` to list skills, or `exit` to leave.");

    loop {
        let Some(line) = read_instruction().await else {
            break;
        };

        let instruction = line.trim();

        if instruction.is_empty() {
            continue;
        }

        if matches!(instruction, "exit" | "quit") {
            break;
        }

        if matches!(instruction, "/skills" | "/help") {
            print!("{}", skills::list(&args.session.dir));

            continue;
        }

        // A failed request should not end the session; show it and let the
        // user try again.
        if let Err(error) = agent.run(&mut conversation, instruction).await {
            eprintln!("error: {error}");
        }
    }

    Ok(ExitCode::SUCCESS)
}

/// Reads one line from the terminal. `None` means end of input.
async fn read_instruction() -> Option<String> {
    tokio::task::spawn_blocking(|| {
        print!("\n> ");

        let _ = io::stdout().flush();

        let mut line = String::new();

        match io::stdin().read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(line),
        }
    })
    .await
    .ok()
    .flatten()
}
