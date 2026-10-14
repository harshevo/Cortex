//! Running shell commands with a time limit.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
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

/// The shell commands are handed to, and the flag that makes it run one.
///
/// Everywhere but Windows that is `sh -c`. On Windows it is the `sh` that
/// comes with Git when there is one, so that the commands a model writes
/// mean the same on every system, and `cmd /C` when there is not.
pub fn shell() -> &'static (PathBuf, &'static str) {
    static SHELL: OnceLock<(PathBuf, &'static str)> = OnceLock::new();

    SHELL.get_or_init(|| {
        if !cfg!(windows) {
            return (PathBuf::from("sh"), "-c");
        }

        match windows_sh() {
            Some(sh) => (sh, "-c"),
            None => (PathBuf::from("cmd"), "/C"),
        }
    })
}

/// What to tell a model about the shell its commands run in.
pub fn shell_note() -> &'static str {
    match shell().1 {
        "-c" => "The command line to run with `sh -c`.",
        _ => {
            "The command line to run with Windows `cmd /C`. There is no `sh` on this machine, \
              so use Windows commands."
        }
    }
}

fn windows_sh() -> Option<PathBuf> {
    let on_path = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .map(|folder| folder.join("sh.exe"));

    let with_git = ["ProgramFiles", "ProgramFiles(x86)", "LocalAppData"]
        .into_iter()
        .filter_map(std::env::var_os)
        .flat_map(|folder| {
            let folder = PathBuf::from(folder);

            [
                folder.join("Git/bin/sh.exe"),
                folder.join("Programs/Git/bin/sh.exe"),
            ]
        });

    on_path
        .chain(with_git)
        .find(|candidate| candidate.is_file())
}

/// Runs `command` through the shell in `cwd`, with the user's own rights.
/// For commands the harness itself decides on, such as an evaluation
/// task's check. What a tool runs goes through [`run_shell_in`].
///
/// On timeout the shell process is killed. Processes it started in the
/// background may survive.
pub async fn run_shell(command: &str, cwd: &Path, timeout: Duration) -> io::Result<CommandOutput> {
    run(Command::new(&shell().0), command, cwd, timeout).await
}

/// Runs `command` through the shell in the project, inside its sandbox
/// when that is on.
pub async fn run_shell_in(
    workspace: &Workspace,
    command: &str,
    timeout: Duration,
) -> io::Result<CommandOutput> {
    let program = shell().0.to_string_lossy();

    run(
        workspace.command(&program),
        command,
        workspace.root(),
        timeout,
    )
    .await
}

async fn run(
    mut shell_command: Command,
    command: &str,
    cwd: &Path,
    timeout: Duration,
) -> io::Result<CommandOutput> {
    let flag = shell().1;

    shell_command.arg(flag);

    // `cmd` reads the rest of its line itself, and does not undo the
    // quoting other Windows programs expect.
    #[cfg(windows)]
    if flag == "/C" {
        shell_command.raw_arg(command);
    } else {
        shell_command.arg(command);
    }

    #[cfg(not(windows))]
    shell_command.arg(command);

    // Without this every command would open a console window of its own.
    #[cfg(windows)]
    shell_command.creation_flags(0x0800_0000);

    let child = shell_command
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
