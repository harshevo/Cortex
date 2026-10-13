//! The environment the app runs in when it is opened from Finder or the
//! Dock.
//!
//! macOS starts such an app with the bare system `PATH`, not the one the
//! person's shell sets up. With it the agent's commands cannot find
//! `cargo`, `gh`, `uv` or `node`, and the mind service is started with the
//! system's Python, which lacks what it needs. So the `PATH` is asked of
//! the person's own shell once, before anything else starts.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// What macOS gives an app that was not started from a shell.
const BARE_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// Shell start-up files may print things of their own, so the answer is
/// read from between two marks.
const MARK: &str = "__CORTEX_PATH__";

/// A shell whose start-up files hang must not keep the window from opening.
const PATIENCE: Duration = Duration::from_secs(5);

/// Takes over the shell's `PATH` when the app was started without one.
/// An app started from a terminal already has it and is left alone.
///
/// Call this first in `main`, while there is only one thread.
pub fn adopt_shell_path() {
    let current = std::env::var("PATH").unwrap_or_default();

    if !current.is_empty() && current != BARE_PATH {
        return;
    }

    let Some(path) = shell_path() else {
        eprintln!("warning: could not read PATH from the shell; commands may not be found");

        return;
    };

    // SAFETY: nothing else is running yet, so nothing reads the
    // environment while it changes.
    unsafe { std::env::set_var("PATH", path) };
}

/// The `PATH` an interactive login shell ends up with.
fn shell_path() -> Option<String> {
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|shell| !shell.is_empty())
        .unwrap_or_else(|| "/bin/zsh".to_string());

    // Login and interactive, so that both kinds of start-up file are
    // read: people set their PATH in either.
    let mut child = Command::new(shell)
        .args(["-ilc", &format!("printf '{MARK}%s{MARK}' \"$PATH\"")])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let started = Instant::now();

    while child.try_wait().ok()?.is_none() {
        if started.elapsed() > PATIENCE {
            let _ = child.kill();
            let _ = child.wait();

            return None;
        }

        std::thread::sleep(Duration::from_millis(20));
    }

    let mut printed = String::new();

    child.stdout.take()?.read_to_string(&mut printed).ok()?;

    between_marks(&printed)
}

fn between_marks(printed: &str) -> Option<String> {
    let (_, rest) = printed.split_once(MARK)?;
    let (path, _) = rest.split_once(MARK)?;

    (!path.is_empty()).then(|| path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_path_is_read_from_between_the_marks() {
        let printed = format!("welcome back\n{MARK}/opt/homebrew/bin:/usr/bin{MARK}\n");

        assert_eq!(
            between_marks(&printed).unwrap(),
            "/opt/homebrew/bin:/usr/bin"
        );
    }

    #[test]
    fn output_without_a_path_gives_nothing() {
        assert_eq!(between_marks("welcome back"), None);
        assert_eq!(between_marks(&format!("{MARK}{MARK}")), None);
    }

    #[test]
    fn the_shell_answers_with_a_path() {
        let path = shell_path().expect("the shell printed no PATH");

        assert!(path.split(':').any(|folder| folder == "/usr/bin"));
    }
}
