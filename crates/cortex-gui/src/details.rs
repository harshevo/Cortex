//! What the details panel says about the folder the agent works in: the
//! computer, the folder, its branch and the changes not yet committed.
//!
//! Everything here is read with `git` and the file system, so it is read
//! off the window's thread. Nothing here can fail loudly: a folder that is
//! not a repository, a missing `git`, a repository that hangs or a file
//! that cannot be read each leave a gap, not an error.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// A repository that takes longer than this to answer is left out.
const PATIENCE: Duration = Duration::from_secs(5);

/// Untracked files beyond these are counted but not read.
const MAX_UNTRACKED_READ: usize = 2_000;
const MAX_UNTRACKED_BYTES: u64 = 2 * 1024 * 1024;

/// What the panel shows about a folder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// The computer's name, when it can be found.
    pub machine: Option<String>,

    /// `None` when the folder is not in a git repository, or git could
    /// not be asked.
    pub git: Option<Git>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Git {
    pub branch: Branch,

    /// `None` when the changes could not be counted.
    pub changes: Option<Changes>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Branch {
    Named(String),

    /// No branch is checked out: the short name of the commit.
    Detached(String),
}

/// The changes since the last commit, untracked files included.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Changes {
    pub files: usize,
    pub added: u64,
    pub removed: u64,

    /// Of `files`, those git does not track yet.
    pub untracked: usize,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.files == 0
    }
}

/// Reads everything about `folder`. Gives up early once `cancelled` is set.
pub fn read(folder: &Path, cancelled: &AtomicBool) -> Snapshot {
    let machine = machine_name();

    if cancelled.load(Ordering::Relaxed) || !folder.is_dir() {
        return Snapshot { machine, git: None };
    }

    Snapshot {
        machine,
        git: read_git(folder, cancelled),
    }
}

static MACHINE_NAME: OnceLock<Option<String>> = OnceLock::new();

/// The computer's name, when it has already been looked up. Never waits,
/// so it is safe to call while drawing.
pub fn cached_machine_name() -> Option<String> {
    MACHINE_NAME.get().cloned().flatten()
}

/// The computer's name, looked up once. Runs a program the first time,
/// so it is called off the window's thread.
pub fn machine_name() -> Option<String> {
    MACHINE_NAME.get_or_init(|| {
        let asked = |program: &str, args: &[&str]| {
            run(program, args, None)
                .map(|out| out.trim().to_string())
                .filter(|name| !name.is_empty())
        };

        let name = if cfg!(target_os = "macos") {
            asked("scutil", &["--get", "ComputerName"])
        } else {
            None
        };

        name.or_else(|| asked("hostname", &[]))
            .or_else(|| std::env::var("HOSTNAME").ok().filter(|name| !name.is_empty()))
            .or_else(|| std::env::var("COMPUTERNAME").ok().filter(|name| !name.is_empty()))
    })
    .clone()
}

fn read_git(folder: &Path, cancelled: &AtomicBool) -> Option<Git> {
    let inside = git(folder, &["rev-parse", "--is-inside-work-tree"])?;

    if inside.trim() != "true" {
        return None;
    }

    let branch = branch(folder)?;

    let changes = if cancelled.load(Ordering::Relaxed) {
        None
    } else {
        changes(folder, cancelled)
    };

    Some(Git { branch, changes })
}

fn branch(folder: &Path) -> Option<Branch> {
    // Answers for a branch with no commits yet, too.
    if let Some(name) = git(folder, &["symbolic-ref", "--quiet", "--short", "HEAD"]) {
        let name = name.trim();

        if !name.is_empty() {
            return Some(Branch::Named(name.to_string()));
        }
    }

    let commit = git(folder, &["rev-parse", "--short", "HEAD"])?;
    let commit = commit.trim();

    (!commit.is_empty()).then(|| Branch::Detached(commit.to_string()))
}

fn changes(folder: &Path, cancelled: &AtomicBool) -> Option<Changes> {
    // A repository with no commits is compared with the empty tree.
    let base = match git(folder, &["rev-parse", "--verify", "--quiet", "HEAD"]) {
        Some(commit) if !commit.trim().is_empty() => "HEAD".to_string(),
        _ => git(folder, &["hash-object", "-t", "tree", "/dev/null"])?
            .trim()
            .to_string(),
    };

    let numstat = git(folder, &["diff", "--numstat", "--no-renames", "-z", &base])?;
    let mut changes = parse_numstat(&numstat);

    if cancelled.load(Ordering::Relaxed) {
        return Some(changes);
    }

    // Untracked files are left out when they cannot be listed: the
    // tracked changes are still worth showing.
    // `diff` counts the whole repository; `ls-files` only lists below the
    // folder it runs in, so it runs at the top, as `diff` counts.
    let root = git(folder, &["rev-parse", "--show-toplevel"])
        .map(|root| PathBuf::from(root.trim_end_matches(['\n', '\r'])))
        .filter(|root| root.is_dir())
        .unwrap_or_else(|| folder.to_path_buf());

    if let Some(listed) = git(&root, &["ls-files", "--others", "--exclude-standard", "-z"]) {
        for (index, name) in listed.split('\0').filter(|name| !name.is_empty()).enumerate() {
            changes.files += 1;
            changes.untracked += 1;

            if index < MAX_UNTRACKED_READ && !cancelled.load(Ordering::Relaxed) {
                changes.added += count_lines(&root.join(name)).unwrap_or(0);
            }
        }
    }

    Some(changes)
}

/// Totals of `git diff --numstat -z` without renames: one
/// `added\tremoved\tpath` record per file, `-` for binary files.
pub fn parse_numstat(output: &str) -> Changes {
    let mut changes = Changes::default();

    for record in output.split('\0') {
        let mut fields = record.splitn(3, '\t');

        let (Some(added), Some(removed), Some(path)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };

        if path.is_empty() {
            continue;
        }

        changes.files += 1;
        changes.added += added.trim().parse::<u64>().unwrap_or(0);
        changes.removed += removed.trim().parse::<u64>().unwrap_or(0);
    }

    changes
}

/// The lines of a text file, as git would count them. `None` for a file
/// that is binary, too large, not a regular file or cannot be read.
fn count_lines(path: &Path) -> Option<u64> {
    let metadata = std::fs::symlink_metadata(path).ok()?;

    if !metadata.is_file() || metadata.len() > MAX_UNTRACKED_BYTES {
        return None;
    }

    let bytes = std::fs::read(path).ok()?;

    lines_in(&bytes)
}

/// `None` for content git would call binary.
pub fn lines_in(bytes: &[u8]) -> Option<u64> {
    if bytes.iter().take(8_000).any(|byte| *byte == 0) {
        return None;
    }

    let newlines = bytes.iter().filter(|byte| **byte == b'\n').count() as u64;
    let unterminated = !bytes.is_empty() && !bytes.ends_with(b"\n");

    Some(newlines + u64::from(unterminated))
}

fn git(folder: &Path, args: &[&str]) -> Option<String> {
    run("git", args, Some(folder))
}

/// Runs `program` and returns what it printed, when it succeeded within
/// [`PATIENCE`]. One that takes longer is killed.
fn run(program: &str, args: &[&str], folder: Option<&Path>) -> Option<String> {
    let mut command = Command::new(program);

    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        // Only reading: never wait on, or leave behind, the index lock.
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0");

    if let Some(folder) = folder {
        command.current_dir(folder);
    }

    let mut child = command.spawn().ok()?;

    // Read on a thread of its own, so that a long answer cannot fill the
    // pipe and hold the program up while it is waited for.
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.read_to_end(&mut out);
        out
    });

    let started = Instant::now();

    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() < PATIENCE => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };

    let out = reader.join().ok()?;

    status
        .filter(|status| status.success())
        .map(|_| String::from_utf8_lossy(&out).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numstat_totals_count_text_and_binary_files() {
        let output = "3\t1\tsrc/a.rs\0-\t-\timage.png\00\t7\told.rs\0";

        assert_eq!(
            parse_numstat(output),
            Changes {
                files: 3,
                added: 3,
                removed: 8,
                untracked: 0,
            }
        );
    }

    #[test]
    fn numstat_of_nothing_is_no_change() {
        assert!(parse_numstat("").is_empty());
        assert!(parse_numstat("\0").is_empty());
    }

    #[test]
    fn lines_are_counted_as_git_counts_them() {
        assert_eq!(lines_in(b""), Some(0));
        assert_eq!(lines_in(b"one\ntwo\n"), Some(2));
        assert_eq!(lines_in(b"one\ntwo"), Some(2));
        assert_eq!(lines_in(b"bin\0ary"), None);
    }

    #[test]
    fn a_folder_outside_any_repository_has_no_git() {
        let folder = tempfile::tempdir().unwrap();
        let snapshot = read(folder.path(), &AtomicBool::new(false));

        assert_eq!(snapshot.git, None);
    }

    #[test]
    fn a_missing_folder_is_not_an_error() {
        let snapshot = read(Path::new("/no/such/folder/anywhere"), &AtomicBool::new(false));

        assert_eq!(snapshot.git, None);
    }

    #[test]
    fn a_repository_reports_its_branch_and_changes() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path();

        let ok = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(path)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        };

        // Without git there is nothing to test.
        if !ok(&["init", "--quiet", "--initial-branch=trunk"]) {
            return;
        }

        // No commits yet: the branch is named and new files count.
        std::fs::write(path.join("new.txt"), "a\nb\n").unwrap();

        let snapshot = read(path, &AtomicBool::new(false));
        let git = snapshot.git.expect("a repository");

        assert_eq!(git.branch, Branch::Named("trunk".into()));
        assert_eq!(
            git.changes,
            Some(Changes {
                files: 1,
                added: 2,
                removed: 0,
                untracked: 1,
            })
        );

        assert!(ok(&["add", "."]));
        assert!(ok(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "--quiet",
            "-m",
            "first"
        ]));

        std::fs::write(path.join("new.txt"), "a\nc\nd\n").unwrap();

        let git = read(path, &AtomicBool::new(false)).git.unwrap();

        assert_eq!(
            git.changes,
            Some(Changes {
                files: 1,
                added: 2,
                removed: 1,
                untracked: 0,
            })
        );
    }
}
