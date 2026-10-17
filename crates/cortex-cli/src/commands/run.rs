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

    if args.session.json {
        let result = serde_json::json!({
            "type": "result",
            "outcome": report.outcome,
            "verified": report.verified,
            "steps": report.steps,
            "usage": report.usage,
            "duration_ms": report.duration.as_millis() as u64,
            "final_text": report.final_text,
        });

        println!("{result}");
    }

    // The console sink has already printed the model's messages. The exit
    // code tells scripts whether the agent finished on its own (0), did
    // not (2), or finished with the project's checks failing (3).
    let code = match (report.outcome, report.verified) {
        (Outcome::Completed, Some(false)) => ExitCode::from(3),
        (Outcome::Completed, _) => ExitCode::SUCCESS,
        (
            Outcome::StepLimit
            | Outcome::Truncated
            | Outcome::Refused
            | Outcome::Stuck
            | Outcome::OverBudget,
            _,
        ) => ExitCode::from(2),
    };

    Ok(code)
}
