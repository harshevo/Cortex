//! Running shell commands with a time limit.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;

use std::collections::VecDeque;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

use crate::workspace::Workspace;

#[derive(Debug, Clone)]
pub struct CommandOutput {
    /// `None` when the command timed out or was killed by a signal.
    pub exit_code: Option<i32>,
    /// Bounded head and tail, with an omission marker for large output.
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
    run(Command::new(&shell().0), command, cwd, timeout, None).await
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
        None,
    )
    .await
}

/// As [`run_shell_in`], with `input` written to the command's standard
/// input.
pub async fn run_shell_in_with_input(
    workspace: &Workspace,
    command: &str,
    timeout: Duration,
    input: Vec<u8>,
) -> io::Result<CommandOutput> {
    let program = shell().0.to_string_lossy();

    run(
        workspace.command(&program),
        command,
        workspace.root(),
        timeout,
        Some(input),
    )
    .await
}

async fn run(
    mut shell_command: Command,
    command: &str,
    cwd: &Path,
    timeout: Duration,
    input: Option<Vec<u8>>,
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

    let stdin = match input {
        Some(_) => Stdio::piped(),
        None => Stdio::null(),
    };

    let mut child = shell_command
        .current_dir(cwd)
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;

    // Written on its own task, so a command that does not read its input
    // cannot hold up the reading of its output.
    if let (Some(input), Some(mut writer)) = (input, child.stdin.take()) {
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;

            let _ = writer.write_all(&input).await;
            let _ = writer.shutdown().await;
        });
    }

    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let mut out = Capture::default();
    let mut err = Capture::default();
    let waited = tokio::time::timeout(timeout, async {
        let (status, (), ()) =
            tokio::try_join!(child.wait(), out.drain(&mut stdout), err.drain(&mut stderr))?;
        Ok::<_, io::Error>(status)
    })
    .await;
    let (exit_code, timed_out) = match waited {
        Ok(status) => (status?.code(), false),
        Err(_) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            (None, true)
        }
    };
    Ok(CommandOutput {
        exit_code,
        stdout: out.finish(),
        stderr: err.finish(),
        timed_out,
    })
}

const CAPTURE_BYTES: usize = 256 * 1024;

#[derive(Default)]
struct Capture {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    total: u64,
}

impl Capture {
    async fn drain(&mut self, reader: &mut (impl AsyncRead + Unpin)) -> io::Result<()> {
        let mut buffer = [0; 16 * 1024];
        loop {
            let read = reader.read(&mut buffer).await?;
            if read == 0 {
                return Ok(());
            }
            self.total = self.total.saturating_add(read as u64);
            let head = (CAPTURE_BYTES / 2 - self.head.len()).min(read);
            self.head.extend_from_slice(&buffer[..head]);
            self.tail.extend(&buffer[head..read]);
            let excess = self.tail.len().saturating_sub(CAPTURE_BYTES / 2);
            self.tail.drain(..excess);
        }
    }

    fn finish(self) -> String {
        let omitted = self
            .total
            .saturating_sub((self.head.len() + self.tail.len()) as u64);
        if omitted == 0 {
            let mut bytes = self.head;
            bytes.extend(self.tail);
            return String::from_utf8_lossy(&bytes).into_owned();
        }
        let mut output = String::from_utf8_lossy(&self.head).into_owned();
        if omitted > 0 {
            output.push_str(&format!("\n... {omitted} output bytes omitted ...\n"));
        }
        output.push_str(&String::from_utf8_lossy(
            &self.tail.into_iter().collect::<Vec<_>>(),
        ));
        output
    }
}

#[cfg(all(test, unix))]
mod capture_tests {
    use super::*;

    #[tokio::test]
    async fn capture_preserves_unicode_across_the_head_boundary() {
        let text = "€".repeat(CAPTURE_BYTES / 4);
        let mut input = text.as_bytes();
        let mut capture = Capture::default();
        capture.drain(&mut input).await.unwrap();
        assert_eq!(capture.finish(), text);
    }

    #[tokio::test]
    async fn noisy_commands_keep_both_ends_and_drain_both_pipes() {
        let result = run_shell("printf BEGIN; head -c 2097152 /dev/zero; printf END; head -c 2097152 /dev/zero >&2; printf ERROR >&2", Path::new("/tmp"), Duration::from_secs(10)).await.unwrap();
        assert!(result.succeeded());
        assert!(result.stdout.starts_with("BEGIN"));
        assert!(result.stdout.ends_with("END"));
        assert!(result.stderr.ends_with("ERROR"));
        for output in [&result.stdout, &result.stderr] {
            assert!(output.len() < CAPTURE_BYTES + 100);
            assert!(output.contains("output bytes omitted"));
        }
    }

    #[tokio::test]
    async fn timeout_keeps_output_and_does_not_wait_for_inherited_pipes() {
        let start = std::time::Instant::now();
        let result = run_shell(
            "printf before; sleep 2",
            Path::new("/tmp"),
            Duration::from_millis(100),
        )
        .await
        .unwrap();
        assert!(result.timed_out);
        assert_eq!(result.stdout, "before");
        assert!(start.elapsed() < Duration::from_secs(1));
    }
}
