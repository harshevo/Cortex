//! `cortex run "<instruction>"`

use std::process::ExitCode;

use anyhow::Result;
use cortex_core::Outcome;

use crate::args::RunArgs;
use crate::session;

pub async fn execute(args: RunArgs) -> Result<ExitCode> {
    let agent = session::build_agent(&args.session).await?;

    let mut conversation = Vec::new();

    let report = agent.run(&mut conversation, &args.instruction).await?;

    // The console sink has already printed the model's messages. The exit
    // code tells scripts whether the agent finished on its own.
    let code = match report.outcome {
        Outcome::Completed => ExitCode::SUCCESS,
        Outcome::StepLimit | Outcome::Truncated | Outcome::Refused => ExitCode::from(2),
    };

    Ok(code)
}
