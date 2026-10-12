//! Running shell commands with a time limit.

use std::io;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

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

/// Runs `command` through `sh -c` in `cwd`.
///
/// On timeout the shell process is killed. Processes it started in the
/// background may survive; proper containment arrives with the sandbox.
pub async fn run_shell(command: &str, cwd: &Path, timeout: Duration) -> io::Result<CommandOutput> {
    let child = Command::new("sh")
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
