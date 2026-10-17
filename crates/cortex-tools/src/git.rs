use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use cortex_core::{Access, Tool, ToolContext, ToolError, ToolSpec};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::input;

const TIMEOUT: Duration = Duration::from_secs(30);

/// The git commands that only look.
const COMMANDS: [&str; 6] = ["status", "diff", "log", "show", "blame", "branch"];

/// Options that would write a file, run a program, or read outside the
/// repository.
const REFUSED: [&str; 6] = [
    "--output",
    "--ext-diff",
    "--textconv",
    "--no-index",
    "--open-files-in-pager",
    "--exec",
];

/// `branch` only lists.
const BRANCH_OPTIONS: [&str; 8] = [
    "-a",
    "-r",
    "-v",
    "-vv",
    "--all",
    "--list",
    "--show-current",
    "--no-color",
];

/// Reads the project's git history and state: what changed, when, by whom
/// and why. It cannot change anything; commits and the like go through
/// the shell, with approval.
pub struct Git;

#[derive(Deserialize)]
struct Input {
    command: String,

    #[serde(default)]
    args: Vec<String>,
}

#[async_trait]
impl Tool for Git {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "git".to_string(),

            description: "Read the project's git state and history without changing \
                          anything: `status`, `diff` (uncommitted changes, or between \
                          revisions), `log` (e.g. args [\"--oneline\", \"-20\", \"--\", \"path\"]), \
                          `show` (a commit, or `REV:path` for a file as it was), `blame` \
                          (who last changed each line, and in which commit), and `branch` \
                          (list). Use it to see what you have changed and to learn why code \
                          is the way it is."
                .to_string(),

            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "enum": COMMANDS,
                    },
                    "args": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Arguments after the command, one per item.",
                    },
                },
                "required": ["command"],
            }),
        }
    }

    fn access(&self) -> Access {
        Access::ReadOnly
    }

    fn describe(&self, input: &Value) -> String {
        let command = input.get("command").and_then(Value::as_str).unwrap_or("?");

        let args: Vec<&str> = input
            .get("args")
            .and_then(Value::as_array)
            .map(|args| args.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();

        format!("git {command} {}", args.join(" "))
            .trim()
            .to_string()
    }

    async fn run(&self, input: Value, context: &ToolContext) -> Result<String, ToolError> {
        let input: Input = input::parse(input)?;

        check(&input)?;

        let root = context.workspace.root();

        let mut command = context.workspace.command("git");

        command
            .args([
                "--no-pager",
                "-c",
                "color.ui=never",
                "-c",
                "core.quotepath=off",
            ])
            .arg(&input.command)
            .args(&input.args)
            .current_dir(root)
            .env("GIT_PAGER", "cat")
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .kill_on_drop(true);

        let output = tokio::time::timeout(TIMEOUT, command.output())
            .await
            .map_err(|_| ToolError::Failed("git did not finish within 30 seconds".to_string()))?
            .map_err(|error| ToolError::Failed(format!("could not run git: {error}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !output.status.success() {
            return Err(ToolError::Failed(format!(
                "git {}: {}",
                input.command,
                stderr.trim()
            )));
        }

        if stdout.trim().is_empty() {
            return Ok(match input.command.as_str() {
                "diff" => "no differences".to_string(),
                _ => "(no output)".to_string(),
            });
        }

        Ok(stdout.into_owned())
    }
}

fn check(input: &Input) -> Result<(), ToolError> {
    if !COMMANDS.contains(&input.command.as_str()) {
        return Err(ToolError::InvalidInput(format!(
            "`{}` is not one of {}; use the shell for anything that changes the repository",
            input.command,
            COMMANDS.join(", ")
        )));
    }

    for arg in &input.args {
        if REFUSED.iter().any(|refused| arg.starts_with(refused)) {
            return Err(ToolError::InvalidInput(format!(
                "`{arg}` is not allowed here"
            )));
        }

        if input.command == "branch" && !BRANCH_OPTIONS.contains(&arg.as_str()) {
            return Err(ToolError::InvalidInput(format!(
                "`branch` only lists here; `{arg}` is not allowed"
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cortex_core::{AutoApprove, Workspace};

    use super::*;

    fn input(command: &str, args: &[&str]) -> Input {
        Input {
            command: command.to_string(),
            args: args.iter().map(|arg| arg.to_string()).collect(),
        }
    }

    #[test]
    fn only_looking_is_allowed() {
        assert!(check(&input("log", &["--oneline", "-5"])).is_ok());
        assert!(check(&input("branch", &["-a"])).is_ok());

        assert!(check(&input("commit", &["-m", "x"])).is_err());
        assert!(check(&input("branch", &["-D", "main"])).is_err());
        assert!(check(&input("diff", &["--output=/tmp/x"])).is_err());
        assert!(check(&input("diff", &["--no-index", "/etc/hosts", "a"])).is_err());
    }

    #[tokio::test]
    async fn reads_the_state_of_a_repository() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();

        let initialised = std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(root)
            .status();

        if !initialised.is_ok_and(|status| status.success()) {
            return;
        }

        std::fs::write(root.join("new.txt"), "x").unwrap();

        let context = ToolContext {
            workspace: Workspace::new(root).unwrap(),
            approver: Arc::new(AutoApprove),
        };

        let status = Git
            .run(
                json!({ "command": "status", "args": ["--short"] }),
                &context,
            )
            .await
            .unwrap();

        assert!(status.contains("new.txt"), "{status}");
    }
}
