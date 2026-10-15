//! `cortex chat`

use std::io::{self, Write};
use std::process::ExitCode;

use anyhow::Result;

use crate::args::ChatArgs;
use crate::commands::skills;
use crate::session;

pub async fn execute(args: ChatArgs) -> Result<ExitCode> {
    let mut agent = None;
    #[cfg(unix)]
    let mut brain = crate::brain_commands::Chat::default();

    let mut conversation = Vec::new();

    println!(
        "Cortex. Type an instruction, `/skills` to list skills, `/compact` to shorten a long \
         conversation, `/teach snn+dann` to teach, `/brain` to talk to the learner, or `exit` to leave."
    );

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

        #[cfg(unix)]
        match brain
            .handle(instruction, &args.session.model, "terminal-brain-chat")
            .await
        {
            Ok(Some(reply)) => {
                println!("{}", reply.text);
                continue;
            }
            Err(error) => {
                eprintln!("error: {error}");
                continue;
            }
            Ok(None) => {}
        }

        if matches!(instruction, "/skills" | "/help") {
            print!("{}", skills::list(&args.session.dir));

            continue;
        }

        if instruction == "/compact" {
            if let Some(agent) = &agent {
                compact(agent, &mut conversation).await;
            }

            continue;
        }

        // A failed request should not end the session; show it and let the
        // user try again.
        if agent.is_none() {
            match session::build_agent(&args.session).await {
                Ok(built) => agent = Some(built),
                Err(error) => {
                    eprintln!("error: {error}");
                    continue;
                }
            }
        }
        if let Err(error) = agent
            .as_ref()
            .unwrap()
            .run(&mut conversation, instruction)
            .await
        {
            eprintln!("error: {error}");
        }
    }

    Ok(ExitCode::SUCCESS)
}

/// Replaces the older part of the conversation with a summary. The
/// agent's console sink says what was done.
async fn compact(agent: &cortex_core::Agent, conversation: &mut Vec<cortex_core::Message>) {
    match agent.compact(conversation).await {
        Ok(Some(compaction)) => println!(
            "The conversation is {} characters shorter; the summary took {} tokens to write.",
            compaction.chars_removed(),
            compaction.usage.total()
        ),

        Ok(None) => println!("The conversation is too short to be worth a summary."),

        Err(error) => eprintln!("error: the conversation was left as it was: {error}"),
    }
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
