//! Running shell commands with a time limit.

use std::io;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use crate::workspace::Workspace;

#[derive(Debug, Clone)]
pub struct CommandOutput {
    /// `None` when the command timed out or was killed by a signal.
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

impl CommandOutput {
    pub fn succeeded(&self) -> bool {
        self.exit_code == Some(0)
    }
}

/// Runs `command` through `sh -c` in `cwd`, with the user's own rights.
/// For commands the harness itself decides on, such as an evaluation
/// task's check. What a tool runs goes through [`run_shell_in`].
///
/// On timeout the shell process is killed. Processes it started in the
/// background may survive.
pub async fn run_shell(command: &str, cwd: &Path, timeout: Duration) -> io::Result<CommandOutput> {
    run(Command::new("sh"), command, cwd, timeout).await
}

/// Runs `command` through `sh -c` in the project, inside its sandbox when
/// that is on.
pub async fn run_shell_in(
    workspace: &Workspace,
    command: &str,
    timeout: Duration,
) -> io::Result<CommandOutput> {
    run(workspace.command("sh"), command, workspace.root(), timeout).await
}

async fn run(
    mut shell: Command,
    command: &str,
    cwd: &Path,
    timeout: Duration,
) -> io::Result<CommandOutput> {
    let child = shell
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;

    let waited = tokio::time::timeout(timeout, child.wait_with_output()).await;

    let Ok(result) = waited else {
        return Ok(CommandOutput {
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: true,
        });
    };

    let output = result?;

    Ok(CommandOutput {
        exit_code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        timed_out: false,
    })
}
