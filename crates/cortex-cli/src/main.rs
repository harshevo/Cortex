//! The `cortex` command.

use std::process::ExitCode;

use clap::Parser;
use cortex_cli::args::{Cli, Command};
use cortex_cli::commands;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    let result = match cli.command {
        Command::Run(args) => commands::run::execute(args).await,
        Command::Chat(args) => commands::chat::execute(args).await,
        Command::Search(args) => commands::search::execute(args).await,
        Command::Context(args) => commands::context::execute(args).await,
        Command::Prepare(args) => commands::prepare::execute(args).await,
        Command::Eval(command) => commands::eval::execute(command).await,
        Command::Skills(args) => commands::skills::execute(args),
        Command::Login => commands::login::execute().await,
        Command::Usage(args) => commands::usage::execute(args),
        Command::Effect(args) => commands::effect::execute(args),
        Command::Undo(args) => commands::checkpoints::undo(args).await,
        Command::Checkpoints(args) => commands::checkpoints::execute(args).await,
        Command::Trace(args) => commands::trace::execute(args).await,
        Command::Learned(args) => commands::learned::execute(args),
    };

    match result {
        Ok(code) => code,

        Err(error) => {
            eprintln!("error: {error:#}");

            ExitCode::FAILURE
        }
    }
}
