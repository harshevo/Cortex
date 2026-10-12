use std::time::Duration;

use async_trait::async_trait;
use cortex_core::process::{self, CommandOutput};
use cortex_core::{Access, Tool, ToolContext, ToolError, ToolSpec};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::checks;
use crate::input;

const DEFAULT_TIMEOUT_SECONDS: u64 = 120;
const MAX_TIMEOUT_SECONDS: u64 = 600;

#[derive(Default)]
pub struct Shell {
    /// Shorten the output of tests, builds and linters. See [`checks`].
    quiet_checks: bool,
}

impl Shell {
    /// A shell that shows only what matters of a check's output: the
    /// summary when it passed, the cleaned-up log when it failed.
    pub fn lean() -> Self {
        Self { quiet_checks: true }
    }
}

#[derive(Deserialize)]
struct Input {
    command: String,

    #[serde(default)]
    timeout_seconds: Option<u64>,
}

#[async_trait]
impl Tool for Shell {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "shell".to_string(),

            description: "Run a shell command in the project directory and return its exit \
                          code, stdout, and stderr. Use it to run tests, builds, linters, and \
                          the program itself. Each call starts a fresh shell, so `cd` and \
                          environment variables do not carry over; commands that wait for input \
                          will fail."
                .to_string(),

            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The command line to run with `sh -c`.",
                    },
                    "timeout_seconds": {
                        "type": "integer",
                        "description": "Time limit (default 120, maximum 600).",
                    },
                },
                "required": ["command"],
            }),
        }
    }

    fn access(&self) -> Access {
        Access::Mutating
    }

    fn describe(&self, input: &Value) -> String {
        let command = input.get("command").and_then(Value::as_str).unwrap_or("?");

        format!("run `{command}`")
    }

    async fn run(&self, input: Value, context: &ToolContext) -> Result<String, ToolError> {
        let input: Input = input::parse(input)?;

        let seconds = input
            .timeout_seconds
            .unwrap_or(DEFAULT_TIMEOUT_SECONDS)
            .clamp(1, MAX_TIMEOUT_SECONDS);

        let output = process::run_shell(
            &input.command,
            context.workspace.root(),
            Duration::from_secs(seconds),
        )
        .await
        .map_err(|error| ToolError::Failed(format!("could not start the command: {error}")))?;

        if output.timed_out {
            return Err(ToolError::Failed(format!(
                "the command did not finish within {seconds} seconds and was stopped"
            )));
        }

        let shorten = self.quiet_checks && checks::is_check_command(&input.command);

        Ok(render(&output, shorten))
    }
}

/// A failing exit code is information for the model, not a tool error.
fn render(output: &CommandOutput, shorten: bool) -> String {
    let status = match output.exit_code {
        Some(code) => format!("exit code: {code}"),
        None => "exit code: none (killed by a signal)".to_string(),
    };

    let mut rendered = status;

    let shown = |stream: &str| match (shorten, output.succeeded()) {
        (false, _) => stream.trim_end().to_string(),
        (true, true) => checks::passed(stream),
        (true, false) => checks::failed(stream),
    };

    if !output.stdout.trim().is_empty() {
        rendered.push_str("\n\nstdout:\n");
        rendered.push_str(&shown(&output.stdout));
    }

    if !output.stderr.trim().is_empty() {
        rendered.push_str("\n\nstderr:\n");
        rendered.push_str(&shown(&output.stderr));
    }

    rendered
}
