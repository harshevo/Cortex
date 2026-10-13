//! Confining the commands a tool starts.
//!
//! A command the model asks for runs with the user's own rights unless
//! something holds it back. With the sandbox on, the operating system lets
//! such a command write only inside the project, the temporary folders and
//! the caches build tools keep, and stops it reading the places where keys
//! are kept. The network is left open: builds download what they depend on.
//!
//! The file tools need none of this. They run inside the agent and reach
//! files only through [`Workspace::resolve`](crate::Workspace::resolve).
//!
//! On macOS the confinement is Seatbelt, through `sandbox-exec`. Other
//! systems have none yet: there the sandbox still keeps the model's keys
//! out of a command's environment, and says once that it does no more.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use tokio::process::Command;

/// Added to a command's output when the sandbox may be why it failed, so
/// the model does not try the same thing again.
pub const DENIAL_NOTE: &str = "note: this command ran in a sandbox. It can write only inside the \
     project directory and temporary directories, and cannot read credential files such as \
     ~/.ssh. If the task needs more than that, say so instead of retrying.";

/// How the system words a refusal by the sandbox.
const DENIAL_SIGN: &str = "Operation not permitted";

/// Keys the agent itself uses to reach its model. No command needs them.
const MODEL_KEYS: [&str; 2] = ["ANTHROPIC_API_KEY", "CORTEX_API_KEY"];

const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// Seatbelt rules. A later rule wins over an earlier one, so the secrets
/// stay shut even in a project that is the home folder itself.
const PROFILE: &str = r##"(version 1)
(allow default)

(deny file-write*)

(allow file-write*
  (subpath (param "ROOT"))
  (subpath "/private/tmp")
  (subpath "/private/var/tmp")
  (subpath "/private/var/folders")
  (literal "/dev/null")
  (literal "/dev/tty")
  (literal "/dev/ptmx")
  (literal "/dev/dtracehelper")
  (regex #"^/dev/ttys[0-9]+$")
  (regex #"^/dev/fd/[0-9]+$"))

; Where build tools keep what they download and compile. Folders that
; hold programs on the PATH (`~/.cargo/bin` and the like) are left out, so
; a command cannot swap one for its own.
(allow file-write*
  (subpath (string-append (param "HOME") "/.cache"))
  (subpath (string-append (param "HOME") "/Library/Caches"))
  (subpath (string-append (param "HOME") "/.cargo/registry"))
  (subpath (string-append (param "HOME") "/.cargo/git"))
  (subpath (string-append (param "HOME") "/.rustup/toolchains"))
  (subpath (string-append (param "HOME") "/.rustup/downloads"))
  (subpath (string-append (param "HOME") "/.rustup/tmp"))
  (subpath (string-append (param "HOME") "/.rustup/update-hashes"))
  (subpath (string-append (param "HOME") "/.npm"))
  (subpath (string-append (param "HOME") "/.node-gyp"))
  (subpath (string-append (param "HOME") "/.yarn"))
  (subpath (string-append (param "HOME") "/.pnpm-store"))
  (subpath (string-append (param "HOME") "/.local/share/pnpm"))
  (subpath (string-append (param "HOME") "/Library/pnpm"))
  (subpath (string-append (param "HOME") "/.bun/install/cache"))
  (subpath (string-append (param "HOME") "/.local/share/uv"))
  (subpath (string-append (param "HOME") "/.gradle"))
  (subpath (string-append (param "HOME") "/.m2"))
  (subpath (string-append (param "HOME") "/.ivy2"))
  (subpath (string-append (param "HOME") "/.sbt"))
  (subpath (string-append (param "HOME") "/go/pkg"))
  (subpath (string-append (param "HOME") "/.nuget/packages"))
  (subpath (string-append (param "HOME") "/.swiftpm"))
  (subpath (string-append (param "HOME") "/Library/Developer/Xcode/DerivedData"))
  (subpath (string-append (param "HOME") "/.cocoapods"))
  (subpath (string-append (param "HOME") "/.gem"))
  (subpath (string-append (param "HOME") "/.bundle/cache"))
  (subpath (string-append (param "HOME") "/.composer/cache"))
  (subpath (string-append (param "HOME") "/.hex"))
  (subpath (string-append (param "HOME") "/.mix"))
  (subpath (string-append (param "HOME") "/.stack"))
  (subpath (string-append (param "HOME") "/.cabal/packages"))
  (subpath (string-append (param "HOME") "/.pub-cache"))
  (subpath (string-append (param "HOME") "/.terraform.d/plugin-cache"))
  (literal (string-append (param "HOME") "/.cargo/.package-cache"))
  (literal (string-append (param "HOME") "/.cargo/.package-cache-mutate"))
  (literal (string-append (param "HOME") "/.cargo/.global-cache"))
  (literal (string-append (param "HOME") "/.rustup/settings.toml")))

; Keys and sign-ins, and the shell files people keep keys in.
(deny file-read* file-write*
  (subpath (string-append (param "HOME") "/.ssh"))
  (subpath (string-append (param "HOME") "/.aws"))
  (subpath (string-append (param "HOME") "/.azure"))
  (subpath (string-append (param "HOME") "/.gnupg"))
  (subpath (string-append (param "HOME") "/.kube"))
  (subpath (string-append (param "HOME") "/.config/gcloud"))
  (literal (string-append (param "HOME") "/.netrc"))
  (literal (string-append (param "HOME") "/.docker/config.json"))
  (literal (string-append (param "HOME") "/.cargo/credentials.toml"))
  (literal (string-append (param "HOME") "/.cortex/codex-auth.json"))
  (literal (string-append (param "HOME") "/.cortex/app.json"))
  (literal (string-append (param "HOME") "/.codex/auth.json"))
  (literal (string-append (param "HOME") "/.zshrc"))
  (literal (string-append (param "HOME") "/.zshenv"))
  (literal (string-append (param "HOME") "/.zprofile"))
  (literal (string-append (param "HOME") "/.bashrc"))
  (literal (string-append (param "HOME") "/.bash_profile"))
  (literal (string-append (param "HOME") "/.profile")))
"##;

/// Whether the commands tools start are confined. On unless switched off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sandbox {
    #[default]
    On,
    Off,
}

impl Sandbox {
    pub fn enabled(enabled: bool) -> Self {
        if enabled { Self::On } else { Self::Off }
    }

    pub fn is_on(self) -> bool {
        self == Self::On
    }

    /// Whether this system can hold a command to the rules above.
    pub fn available() -> bool {
        cfg!(target_os = "macos") && Path::new(SANDBOX_EXEC).exists() && home().is_some()
    }

    /// Whether commands really are confined: switched on, on a system
    /// that can do it.
    pub fn confines(self) -> bool {
        self.is_on() && Self::available()
    }

    /// One line for whoever starts a session, saying what holds.
    pub fn summary(self, root: &Path) -> String {
        match (self, Self::available()) {
            (Self::Off, _) => "sandbox: off; commands run with your own rights".to_string(),

            (Self::On, true) => format!(
                "sandbox: commands can write only in {} and temporary folders",
                root.display()
            ),

            (Self::On, false) => {
                "sandbox: this system has none yet, so commands are not confined".to_string()
            }
        }
    }

    /// A command that runs `program`, confined to writing under `root`
    /// when the sandbox is on. `root` must be a real path with no links
    /// in it, as [`Workspace::root`](crate::Workspace::root) is.
    pub fn command(self, program: &str, root: &Path) -> Command {
        if !self.is_on() {
            return Command::new(program);
        }

        let mut command = match home().filter(|_| Self::available()) {
            Some(home) => {
                let mut command = Command::new(SANDBOX_EXEC);

                command
                    .arg("-p")
                    .arg(PROFILE)
                    .arg("-D")
                    .arg(parameter("ROOT", root))
                    .arg("-D")
                    .arg(parameter("HOME", home))
                    .arg(program);

                command
            }

            None => {
                warn_unconfined();

                Command::new(program)
            }
        };

        for key in MODEL_KEYS {
            command.env_remove(key);
        }

        command
    }

    /// Whether `errors` is the sandbox saying the program it was asked to
    /// run does not exist. Unconfined, that fails when the command is
    /// started; confined, the sandbox starts and reports it instead.
    pub fn program_missing(self, errors: &str) -> bool {
        self.confines()
            && errors.starts_with("sandbox-exec: execvp() of")
            && errors.contains("No such file or directory")
    }

    /// The note to put after a failed command's output, when the sandbox
    /// looks like the reason it failed.
    pub fn denial_note(self, succeeded: bool, output: &str) -> Option<&'static str> {
        (self.confines() && !succeeded && output.contains(DENIAL_SIGN)).then_some(DENIAL_NOTE)
    }
}

fn parameter(name: &str, path: &Path) -> std::ffi::OsString {
    let mut parameter = std::ffi::OsString::from(format!("{name}="));

    parameter.push(path);

    parameter
}

/// The home folder as the system itself names it. Seatbelt compares real
/// paths, so a home reached through a link has to be resolved first.
fn home() -> Option<&'static Path> {
    static HOME: OnceLock<Option<PathBuf>> = OnceLock::new();

    HOME.get_or_init(|| std::env::home_dir()?.canonicalize().ok())
        .as_deref()
}

fn warn_unconfined() {
    static WARNED: OnceLock<()> = OnceLock::new();

    WARNED.get_or_init(|| {
        eprintln!(
            "warning: the sandbox is on, but this system has none yet. Commands are not confined."
        );
    });
}

#[cfg(test)]
mod tests {
    use std::process::Stdio;

    use super::*;

    async fn run(sandbox: Sandbox, root: &Path, script: &str) -> (bool, String) {
        let output = sandbox
            .command("sh", root)
            .arg("-c")
            .arg(script)
            .current_dir(root)
            .stdin(Stdio::null())
            .output()
            .await
            .unwrap();

        (
            output.status.success(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }

    /// A folder outside every place the sandbox lets a command write.
    fn outside() -> Option<tempfile::TempDir> {
        let home = home()?;

        tempfile::Builder::new()
            .prefix(".cortex-sandbox-test-")
            .tempdir_in(home)
            .ok()
    }

    #[tokio::test]
    async fn a_command_writes_inside_the_project() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();

        let (succeeded, _) = run(Sandbox::On, &root, "mkdir src && echo hi > src/new.txt").await;

        assert!(succeeded);
        assert!(root.join("src/new.txt").exists());
    }

    #[tokio::test]
    async fn a_command_cannot_write_outside_the_project() {
        if !Sandbox::available() {
            return;
        }

        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();

        let Some(elsewhere) = outside() else { return };
        let target = elsewhere.path().join("written.txt");

        let script = format!("echo hi > '{}'", target.display());

        let (succeeded, errors) = run(Sandbox::On, &root, &script).await;

        assert!(!succeeded);
        assert!(!target.exists());
        assert!(Sandbox::On.denial_note(succeeded, &errors).is_some());

        // The same command, unconfined, is what the switch gives back.
        let (succeeded, _) = run(Sandbox::Off, &root, &script).await;

        assert!(succeeded);
        assert!(target.exists());
    }

    #[test]
    fn the_model_keys_are_kept_from_a_command() {
        let command = Sandbox::On.command("sh", Path::new("/"));

        for key in MODEL_KEYS {
            let removed = command
                .as_std()
                .get_envs()
                .any(|(name, value)| name == key && value.is_none());

            assert!(removed, "{key} is passed on");
        }
    }

    #[test]
    fn a_passing_command_gets_no_note() {
        assert!(
            Sandbox::On
                .denial_note(true, "Operation not permitted")
                .is_none()
        );
        assert!(
            Sandbox::Off
                .denial_note(false, "Operation not permitted")
                .is_none()
        );
    }
}
