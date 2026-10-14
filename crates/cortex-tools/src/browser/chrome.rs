//! Finding and starting the browser.
//!
//! Any browser built on Chromium will do: Chrome, Chromium, Edge or Brave.
//! It is started with a profile of its own in a temporary folder, so it
//! shares no logins, history or extensions with the person's browser, and
//! it is controlled through two pipes rather than a network port, so
//! nothing else on the machine can reach it. When the pipes close, because
//! this value was dropped or the whole program ended, the browser exits.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use cortex_core::ToolError;
use tokio::process::Child;

use super::cdp::{Connection, OnEvent};

/// Names a browser's program in this variable to use that one.
pub const PROGRAM_VARIABLE: &str = "CORTEX_BROWSER";

const MAC_APPLICATIONS: [&str; 5] = [
    "Google Chrome.app/Contents/MacOS/Google Chrome",
    "Chromium.app/Contents/MacOS/Chromium",
    "Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
    "Brave Browser.app/Contents/MacOS/Brave Browser",
    "Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary",
];

const WINDOWS_PROGRAMS: [&str; 4] = [
    "Google/Chrome/Application/chrome.exe",
    "Chromium/Application/chrome.exe",
    "Microsoft/Edge/Application/msedge.exe",
    "BraveSoftware/Brave-Browser/Application/brave.exe",
];

const PROGRAM_NAMES: [&str; 8] = [
    "msedge",
    "google-chrome",
    "google-chrome-stable",
    "chromium",
    "chromium-browser",
    "microsoft-edge",
    "brave-browser",
    "chrome",
];

/// The size of the page when the browser starts.
pub const WIDTH: u32 = 1280;
pub const HEIGHT: u32 = 800;

/// A running browser.
pub struct Chrome {
    pub connection: Connection,
    child: Child,
    profile: PathBuf,
}

impl Chrome {
    /// The folder the browser keeps its profile in.
    #[cfg(test)]
    pub fn profile(&self) -> &Path {
        &self.profile
    }
}

impl Drop for Chrome {
    fn drop(&mut self) {
        let _ = self.child.start_kill();

        // The browser's helper processes go on writing to the profile for
        // a moment after it is killed, and a folder that is being written
        // to does not stay removed. A second is far more than they take.
        for _ in 0..40 {
            let _ = std::fs::remove_dir_all(&self.profile);

            if !self.profile.exists() {
                break;
            }

            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }
}

/// The browser's program: the one named in `CORTEX_BROWSER`, or the first
/// one found in the usual places.
pub fn find_program() -> Result<PathBuf, ToolError> {
    if let Some(named) = std::env::var_os(PROGRAM_VARIABLE).filter(|named| !named.is_empty()) {
        let named = PathBuf::from(named);

        if named.is_file() {
            return Ok(named);
        }

        return Err(ToolError::Failed(format!(
            "`{PROGRAM_VARIABLE}` names {}, which is not a file",
            named.display()
        )));
    }

    let home = std::env::home_dir();

    let folders = [
        Some(PathBuf::from("/Applications")),
        home.map(|home| home.join("Applications")),
    ];

    let applications = folders
        .into_iter()
        .flatten()
        .flat_map(|folder| MAC_APPLICATIONS.map(|application| folder.join(application)));

    // Where Windows keeps programs. Edge is part of Windows itself, so
    // one of these is there on any machine.
    let installed = ["ProgramFiles", "ProgramFiles(x86)", "LocalAppData"]
        .into_iter()
        .filter(|_| cfg!(windows))
        .filter_map(std::env::var_os)
        .flat_map(|folder| WINDOWS_PROGRAMS.map(|program| PathBuf::from(&folder).join(program)));

    let on_path = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .flat_map(|folder| {
            PROGRAM_NAMES.map(|name| folder.join(format!("{name}{}", std::env::consts::EXE_SUFFIX)))
        });

    let applications = applications.chain(installed);

    applications
        .chain(on_path)
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| {
            ToolError::Failed(format!(
                "no browser was found. Install Chrome, Chromium, Edge or Brave, or set \
                 `{PROGRAM_VARIABLE}` to the program of one"
            ))
        })
}

/// The browser's command line, apart from how it is handed its pipes.
fn command(program: &Path, profile: &Path, show_window: bool) -> tokio::process::Command {
    use std::process::Stdio;

    let mut command = tokio::process::Command::new(program);

    command
        .arg("--remote-debugging-pipe")
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(format!("--window-size={WIDTH},{HEIGHT}"))
        .args([
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-extensions",
            "--disable-sync",
            "--disable-background-networking",
            "--disable-component-update",
            "--disable-default-apps",
            "--disable-features=Translate,MediaRouter",
            "--mute-audio",
            // Never ask for, or look in, the person's keychain.
            "--password-store=basic",
            "--use-mock-keychain",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);

    if !show_window {
        command.arg("--headless=new");
    }

    command
}

fn failed(what: &str, error: std::io::Error) -> ToolError {
    ToolError::Failed(format!("could not {what}: {error}"))
}

/// Starts the browser at `program`. With `show_window` it opens a window
/// the person can watch; otherwise it runs out of sight.
#[cfg(unix)]
pub fn launch(program: &Path, show_window: bool, on_event: OnEvent) -> Result<Chrome, ToolError> {
    use std::os::fd::{AsRawFd, OwnedFd};

    let profile = new_profile().map_err(|error| failed("make a folder for the browser", error))?;

    let (browser_reads, we_write) =
        std::io::pipe().map_err(|error| failed("open a pipe to the browser", error))?;
    let (we_read, browser_writes) =
        std::io::pipe().map_err(|error| failed("open a pipe to the browser", error))?;

    // The browser expects its ends at descriptors 3 and 4. Moving ours well
    // clear of those first means putting one in place cannot overwrite the
    // other.
    let browser_reads = lift(OwnedFd::from(browser_reads))
        .map_err(|error| failed("prepare a pipe for the browser", error))?;
    let browser_writes = lift(OwnedFd::from(browser_writes))
        .map_err(|error| failed("prepare a pipe for the browser", error))?;

    let (reads, writes) = (browser_reads.as_raw_fd(), browser_writes.as_raw_fd());

    let mut command = command(program, &profile, show_window);

    command.arg("about:blank");

    // SAFETY: this runs in the new process between fork and exec, where
    // only async-signal-safe calls are allowed; `dup2` is one. The copies
    // it makes stay open across the exec, unlike the originals.
    unsafe {
        command.pre_exec(move || {
            if libc::dup2(reads, 3) < 0 || libc::dup2(writes, 4) < 0 {
                return Err(std::io::Error::last_os_error());
            }

            Ok(())
        });
    }

    let child = command.spawn().map_err(|error| {
        let _ = std::fs::remove_dir_all(&profile);

        failed(
            &format!("start the browser at {}", program.display()),
            error,
        )
    })?;

    // Ours to close: while a copy is open here, the browser going away
    // would not show as the end of the pipe.
    drop(browser_reads);
    drop(browser_writes);

    Ok(Chrome {
        connection: Connection::open(we_write, we_read, on_event),
        child,
        profile,
    })
}

/// Starts the browser at `program`. With `show_window` it opens a window
/// the person can watch; otherwise it runs out of sight.
///
/// Windows has no numbered descriptors to put the pipes at. The browser
/// is told the two handles by value instead, and inherits them.
#[cfg(windows)]
pub fn launch(program: &Path, show_window: bool, on_event: OnEvent) -> Result<Chrome, ToolError> {
    use std::os::windows::io::AsRawHandle;

    use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};

    let profile = new_profile().map_err(|error| failed("make a folder for the browser", error))?;

    let (browser_reads, we_write) =
        std::io::pipe().map_err(|error| failed("open a pipe to the browser", error))?;
    let (we_read, browser_writes) =
        std::io::pipe().map_err(|error| failed("open a pipe to the browser", error))?;

    let (reads, writes) = (browser_reads.as_raw_handle(), browser_writes.as_raw_handle());

    for handle in [reads, writes] {
        // SAFETY: both handles are open, and owned here until the
        // browser has started.
        let marked = unsafe { SetHandleInformation(handle as _, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) };

        if marked == 0 {
            return Err(failed(
                "prepare a pipe for the browser",
                std::io::Error::last_os_error(),
            ));
        }
    }

    let mut command = command(program, &profile, show_window);

    command
        .arg(format!(
            "--remote-debugging-io-pipes={},{}",
            reads as usize, writes as usize
        ))
        .arg("about:blank");

    let child = command.spawn().map_err(|error| {
        let _ = std::fs::remove_dir_all(&profile);

        failed(
            &format!("start the browser at {}", program.display()),
            error,
        )
    })?;

    // Ours to close: while a copy is open here, the browser going away
    // would not show as the end of the pipe.
    drop(browser_reads);
    drop(browser_writes);

    Ok(Chrome {
        connection: Connection::open(we_write, we_read, on_event),
        child,
        profile,
    })
}

#[cfg(not(any(unix, windows)))]
pub fn launch(_: &Path, _: bool, _: OnEvent) -> Result<Chrome, ToolError> {
    Err(ToolError::Failed(
        "the browser tools do not work on this operating system yet".to_string(),
    ))
}

/// A copy of `descriptor` numbered 10 or above, closed in any program this
/// one starts.
#[cfg(unix)]
fn lift(descriptor: std::os::fd::OwnedFd) -> std::io::Result<std::os::fd::OwnedFd> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    // SAFETY: `descriptor` is open for the length of the call, and the
    // number that comes back is a new descriptor nothing else owns.
    unsafe {
        let lifted = libc::fcntl(descriptor.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 10);

        if lifted < 0 {
            return Err(std::io::Error::last_os_error());
        }

        Ok(OwnedFd::from_raw_fd(lifted))
    }
}

/// An empty folder for the browser's profile.
fn new_profile() -> std::io::Result<PathBuf> {
    static COUNT: AtomicU32 = AtomicU32::new(0);

    let name = format!(
        "cortex-browser-{}-{}",
        std::process::id(),
        COUNT.fetch_add(1, Ordering::Relaxed)
    );

    let profile = std::env::temp_dir().join(name);

    // Left over from a process that had the same number and did not get
    // to tidy up.
    let _ = std::fs::remove_dir_all(&profile);

    std::fs::create_dir_all(&profile)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        std::fs::set_permissions(&profile, std::fs::Permissions::from_mode(0o700))?;
    }

    Ok(profile)
}
