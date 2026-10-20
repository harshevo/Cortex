//! Bringing the work of parallel jobs back together.
//!
//! Each job works on a branch of its own (see [`crate::jobs`]), so jobs
//! running at once cannot overwrite each other. What can still go wrong
//! is the meeting of their work, and this is where it is looked after:
//!
//! - While jobs run, [`changed_files`] says what each one has touched,
//!   and [`overlaps`] which of them are touching the same files, so a
//!   collision is seen, and the jobs told, before it is a conflict.
//! - When a job is done, [`check`] tries the merge without touching any
//!   checkout (`git merge-tree`) and names the files that would conflict.
//! - [`merge`] then merges one branch, only when that is clean, the
//!   project is not in the middle of another merge, and nothing the
//!   person has not committed would be written over. A merge that fails
//!   anyway is undone, so the project is left as it was.
//! - A branch that conflicts is brought up to date in its own worktree
//!   instead ([`merge_into_worktree`]), where an agent resolves it
//!   without the person's checkout ever holding conflict markers.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Stdio;

use tokio::process::Command;

use crate::jobs::Overlap;

/// Cortex's own folder, never part of a job's work.
const OWN_FOLDER: &str = ".cortex/";

/// What a trial merge found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Check {
    Clean,

    /// These files would conflict.
    Conflicts(Vec<String>),

    /// The branch has nothing the project does not have already.
    AlreadyIn,
}

/// What came of [`merge`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Merged {
    /// Merged, as this commit.
    Done(String),

    AlreadyIn,

    /// These files conflict. Nothing was changed.
    Conflicts(Vec<String>),

    /// The merge was not tried, or was undone, for this reason. It may
    /// go through later, once the reason is gone.
    Waiting(String),
}

/// The commit checked out at `root`.
pub async fn head(root: &Path) -> Result<String, String> {
    git(root, &["rev-parse", "HEAD"])
        .await
        .map(|out| out.trim().to_string())
}

/// The branch checked out at `root`, or `None` when detached.
pub async fn current_branch(root: &Path) -> Option<String> {
    git(root, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .await
        .ok()
        .map(|out| out.trim().to_string())
        .filter(|branch| !branch.is_empty())
}

/// Whether `root` is inside a git repository that has a commit.
pub async fn has_history(root: &Path) -> bool {
    head(root).await.is_ok()
}

/// The files changed in the checkout at `root` since `base`: committed,
/// staged, not staged and new, relative to the top of the repository.
pub async fn changed_files(root: &Path, base: &str) -> Result<Vec<String>, String> {
    let committed = git(root, &["diff", "--name-only", "-z", base, "HEAD"]).await?;
    let status = git(root, &["status", "--porcelain=v1", "-z", "--untracked-files=all"]).await?;

    let mut files: BTreeSet<String> = split_nul(&committed).collect();

    files.extend(porcelain_paths(&status));

    Ok(files
        .into_iter()
        .filter(|file| !file.starts_with(OWN_FOLDER))
        .collect())
}

/// For each job, the other jobs that change some of the same files.
/// Only pairs that share a file are listed, each from both sides.
pub fn overlaps(changes: &[(String, Vec<String>)]) -> BTreeMap<String, Vec<Overlap>> {
    let mut found: BTreeMap<String, Vec<Overlap>> = BTreeMap::new();

    for (index, (job, files)) in changes.iter().enumerate() {
        let mine: BTreeSet<&String> = files.iter().collect();

        for (other, theirs) in changes.iter().skip(index + 1) {
            let shared: Vec<String> = theirs
                .iter()
                .filter(|file| mine.contains(file))
                .cloned()
                .collect();

            if shared.is_empty() {
                continue;
            }

            found.entry(job.clone()).or_default().push(Overlap {
                job: other.clone(),
                files: shared.clone(),
            });

            found.entry(other.clone()).or_default().push(Overlap {
                job: job.clone(),
                files: shared,
            });
        }
    }

    found
}

/// Commits everything changed in the worktree at `root`, Cortex's own
/// folder aside. Returns the commit, or `None` when nothing changed.
pub async fn commit_all(root: &Path, message: &str) -> Result<Option<String>, String> {
    stage_all(root).await?;

    let staged = git_status(root, &["diff", "--cached", "--quiet"]).await?;

    if staged {
        return Ok(None);
    }

    let mut args = identity(root).await;

    args.extend(["commit", "--no-verify", "--quiet", "-m", message].map(String::from));

    git_owned(root, &args).await?;

    head(root).await.map(Some)
}

/// Stages everything changed in the checkout at `root` except Cortex's
/// own folders, which are put back as the last commit has them. Naming
/// them in the `git add` instead would fail wherever they are ignored.
async fn stage_all(root: &Path) -> Result<(), String> {
    git(root, &["add", "--all", "--", "."]).await?;

    git(
        root,
        &["reset", "--quiet", "--", ":(glob).cortex/**", ":(glob)**/.cortex/**"],
    )
    .await
    .map(|_| ())
}

/// Tries merging `branch` into what is checked out at `root`, touching
/// nothing.
pub async fn check(root: &Path, branch: &str) -> Result<Check, String> {
    if git_status(root, &["merge-base", "--is-ancestor", branch, "HEAD"]).await? {
        return Ok(Check::AlreadyIn);
    }

    let output = Command::new("git")
        .args([
            "merge-tree",
            "--write-tree",
            "--name-only",
            "--no-messages",
            "HEAD",
            branch,
        ])
        .current_dir(root)
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|error| format!("could not run git: {error}"))?;

    match output.status.code() {
        Some(0) => Ok(Check::Clean),

        // The first line names the tree written; the rest are the files
        // with conflicts.
        Some(1) => {
            let listed = String::from_utf8_lossy(&output.stdout);

            let files: BTreeSet<String> = listed
                .lines()
                .skip(1)
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(String::from)
                .collect();

            Ok(Check::Conflicts(files.into_iter().collect()))
        }

        _ => Err(format!(
            "git merge-tree failed (it needs git 2.38 or later): {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )),
    }
}

/// Merges `branch` into what is checked out at `root`, as one merge
/// commit, when it can be done without conflicts and without writing over
/// anything uncommitted. Otherwise the project is left untouched.
pub async fn merge(root: &Path, branch: &str, message: &str) -> Result<Merged, String> {
    if let Some(reason) = busy(root).await {
        return Ok(Merged::Waiting(reason));
    }

    if current_branch(root).await.is_none() {
        return Ok(Merged::Waiting(
            "the project is not on a branch (detached HEAD)".to_string(),
        ));
    }

    match check(root, branch).await? {
        Check::AlreadyIn => return Ok(Merged::AlreadyIn),
        Check::Conflicts(files) => return Ok(Merged::Conflicts(files)),
        Check::Clean => {}
    }

    // What the merge would write, against what the person has changed.
    let incoming = git(root, &["diff", "--name-only", "-z", &format!("HEAD...{branch}")]).await?;
    let incoming: BTreeSet<String> = split_nul(&incoming).collect();

    let status = git(root, &["status", "--porcelain=v1", "-z", "--untracked-files=all"]).await?;

    let touched: Vec<String> = porcelain_paths(&status)
        .into_iter()
        .filter(|file| incoming.contains(file))
        .collect();

    if !touched.is_empty() {
        return Ok(Merged::Waiting(format!(
            "uncommitted changes in the project to {}; commit or stash them first",
            touched.join(", ")
        )));
    }

    let mut args = identity(root).await;

    args.extend(
        ["merge", "--no-ff", "--no-edit", "--quiet", "-m", message, branch].map(String::from),
    );

    if let Err(error) = git_owned(root, &args).await {
        // Only a merge this call began is undone.
        if git_status(root, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]).await == Ok(true) {
            let _ = git(root, &["merge", "--abort"]).await;
        }

        return Ok(Merged::Waiting(format!("git refused the merge: {error}")));
    }

    head(root).await.map(Merged::Done)
}

/// Merges `commit` into the job's own worktree at `root`, to bring its
/// branch up to date with the project. Returns the files left with
/// conflict markers for an agent to resolve; none when it merged cleanly,
/// in which case the merge is committed.
pub async fn merge_into_worktree(root: &Path, commit: &str) -> Result<Vec<String>, String> {
    if let Some(reason) = busy(root).await {
        return Err(reason);
    }

    let mut args = identity(root).await;

    args.extend(["merge", "--no-ff", "--no-edit", "--quiet", commit].map(String::from));

    if git_owned(root, &args).await.is_ok() {
        return Ok(Vec::new());
    }

    let conflicted = git(root, &["diff", "--name-only", "-z", "--diff-filter=U"]).await?;
    let files: Vec<String> = split_nul(&conflicted).collect();

    if files.is_empty() {
        // It failed for another reason; leave the worktree as it was.
        if git_status(root, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]).await == Ok(true) {
            let _ = git(root, &["merge", "--abort"]).await;
        }

        return Err("the project's commit could not be merged into the job".to_string());
    }

    Ok(files)
}

/// Concludes a merge left in progress in the worktree at `root`, once
/// its conflicts are resolved: stages the resolution and commits it.
/// Returns the files that still conflict, when any do, and commits
/// nothing then. With no merge in progress it does nothing.
pub async fn finish_merge(root: &Path) -> Result<Vec<String>, String> {
    if git_status(root, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]).await != Ok(true) {
        return Ok(Vec::new());
    }

    let left: Vec<String> = unresolved(root)
        .await
        .into_iter()
        .filter(|file| {
            std::fs::read_to_string(root.join(file)).map_or(true, |text| has_markers(&text))
        })
        .collect();

    if !left.is_empty() {
        return Ok(left);
    }

    stage_all(root).await?;

    // Any file still marked as conflicting has no markers left: staging
    // it above was its resolution.
    let left = unresolved(root).await;

    if !left.is_empty() {
        return Ok(left);
    }

    let mut args = identity(root).await;

    args.extend(["commit", "--no-verify", "--no-edit", "--quiet"].map(String::from));

    git_owned(root, &args).await?;

    Ok(Vec::new())
}

/// Whether `text` still holds a conflict's markers, each at the start of
/// a line.
fn has_markers(text: &str) -> bool {
    let mut open = false;

    for line in text.lines() {
        if line.starts_with("<<<<<<< ") {
            open = true;
        } else if open && line.starts_with(">>>>>>> ") {
            return true;
        }
    }

    false
}

/// Whether the checkout at `root` has a merge or rebase under way.
pub async fn in_progress(root: &Path) -> bool {
    busy(root).await.is_some()
}

/// Files that still hold conflict markers in the checkout at `root`.
pub async fn unresolved(root: &Path) -> Vec<String> {
    let listed = git(root, &["diff", "--name-only", "-z", "--diff-filter=U"])
        .await
        .unwrap_or_default();

    split_nul(&listed).collect()
}

/// Removes the worktree at `path` and, when `branch` is given, deletes
/// that branch. `force` drops a branch that was never merged.
pub async fn remove_worktree(
    root: &Path,
    path: &Path,
    branch: Option<&str>,
    force: bool,
) -> Result<(), String> {
    if path.exists() {
        git(
            root,
            &["worktree", "remove", "--force", &path.to_string_lossy()],
        )
        .await?;
    }

    let _ = git(root, &["worktree", "prune"]).await;

    if let Some(branch) = branch {
        let flag = if force { "-D" } else { "-d" };

        git(root, &["branch", flag, branch]).await?;
    }

    Ok(())
}

async fn busy(root: &Path) -> Option<String> {
    for (reference, what) in [
        ("MERGE_HEAD", "a merge"),
        ("REBASE_HEAD", "a rebase"),
        ("CHERRY_PICK_HEAD", "a cherry-pick"),
        ("REVERT_HEAD", "a revert"),
    ] {
        if git_status(root, &["rev-parse", "-q", "--verify", reference]).await == Ok(true) {
            return Some(format!("{what} is in progress in {}", root.display()));
        }
    }

    for folder in ["rebase-merge", "rebase-apply"] {
        let path = git(root, &["rev-parse", "--path-format=absolute", "--git-path", folder]).await;

        if path.is_ok_and(|path| Path::new(path.trim()).exists()) {
            return Some(format!("a rebase is in progress in {}", root.display()));
        }
    }

    None
}

/// `-c` settings that give a commit an author when the repository has
/// none set up, so that a job can commit on any machine.
async fn identity(root: &Path) -> Vec<String> {
    let mut args = Vec::new();

    for (key, fallback) in [("user.name", "Cortex"), ("user.email", "cortex@localhost")] {
        let set = git(root, &["config", key])
            .await
            .is_ok_and(|value| !value.trim().is_empty());

        if !set {
            args.extend(["-c".to_string(), format!("{key}={fallback}")]);
        }
    }

    args
}

/// The paths in `git status --porcelain=v1 -z` output. A rename gives
/// both its new and old path.
fn porcelain_paths(status: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let mut entries = status.split('\0').filter(|entry| !entry.is_empty());

    while let Some(entry) = entries.next() {
        if entry.len() < 4 {
            continue;
        }

        let code = &entry[..2];

        paths.push(entry[3..].to_string());

        if (code.contains('R') || code.contains('C'))
            && let Some(from) = entries.next()
        {
            paths.push(from.to_string());
        }
    }

    paths
}

fn split_nul(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split('\0')
        .map(str::trim_end)
        .filter(|entry| !entry.is_empty())
        .map(String::from)
}

async fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|error| format!("could not run git: {error}"))?;

    match output.status.success() {
        true => Ok(String::from_utf8_lossy(&output.stdout).into_owned()),
        false => Err(String::from_utf8_lossy(&output.stderr).trim().to_string()),
    }
}

async fn git_owned(root: &Path, args: &[String]) -> Result<String, String> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();

    git(root, &args).await
}

/// Runs a git command whose answer is its exit status: `Ok(true)` for 0,
/// `Ok(false)` for 1, an error otherwise.
async fn git_status(root: &Path, args: &[&str]) -> Result<bool, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|error| format!("could not run git: {error}"))?;

    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(String::from_utf8_lossy(&output.stderr).trim().to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn run(root: &Path, args: &[&str]) -> bool {
        std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .current_dir(root)
            .output()
            .is_ok_and(|output| output.status.success())
    }

    /// A repository with one commit holding `a.txt` and `b.txt`, or
    /// `None` where git cannot be run.
    fn repository() -> Option<(tempfile::TempDir, PathBuf)> {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("app");

        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        std::fs::write(root.join("b.txt"), "b\n").unwrap();

        let ready = run(&root, &["init", "--quiet", "-b", "main"])
            && run(&root, &["add", "."])
            && run(&root, &["commit", "-qm", "start"]);

        ready.then_some((parent, root))
    }

    #[test]
    fn overlapping_files_are_reported_from_both_sides() {
        let found = overlaps(&[
            ("job-1".into(), vec!["a.rs".into(), "b.rs".into()]),
            ("job-2".into(), vec!["b.rs".into()]),
            ("job-3".into(), vec!["c.rs".into()]),
        ]);

        assert_eq!(found["job-1"][0].job, "job-2");
        assert_eq!(found["job-2"][0].files, vec!["b.rs".to_string()]);
        assert!(!found.contains_key("job-3"));
    }

    #[test]
    fn porcelain_paths_include_both_sides_of_a_rename() {
        let status = "R  new.rs\0old.rs\0 M a b.rs\0?? c.rs\0";

        assert_eq!(
            porcelain_paths(status),
            vec!["new.rs", "old.rs", "a b.rs", "c.rs"]
        );
    }

    #[tokio::test]
    async fn clean_work_merges_and_conflicting_work_waits_untouched() {
        let Some((_parent, root)) = repository() else {
            return;
        };

        let base = head(&root).await.unwrap();

        // Two jobs, each in a worktree of its own.
        let one = root.with_file_name("one");
        let two = root.with_file_name("two");

        assert!(run(&root, &["worktree", "add", "-q", "-b", "job-1", &one.to_string_lossy()]));
        assert!(run(&root, &["worktree", "add", "-q", "-b", "job-2", &two.to_string_lossy()]));

        std::fs::write(one.join("a.txt"), "one\nTWO\nthree\n").unwrap();
        std::fs::write(one.join("new.txt"), "new\n").unwrap();
        std::fs::write(two.join("a.txt"), "one\n2\nthree\n").unwrap();

        assert_eq!(
            changed_files(&one, &base).await.unwrap(),
            vec!["a.txt", "new.txt"]
        );

        let found = overlaps(&[
            ("job-1".into(), changed_files(&one, &base).await.unwrap()),
            ("job-2".into(), changed_files(&two, &base).await.unwrap()),
        ]);

        assert_eq!(found["job-1"][0].files, vec!["a.txt".to_string()]);

        assert!(commit_all(&one, "job 1").await.unwrap().is_some());
        // Cortex's own folder is left out, ignored or not.
        std::fs::create_dir_all(two.join(".cortex")).unwrap();
        std::fs::write(two.join(".cortex/state.json"), "{}").unwrap();

        assert!(commit_all(&two, "job 2").await.unwrap().is_some());
        assert!(!run(&two, &["cat-file", "-e", "HEAD:.cortex/state.json"]));

        std::fs::write(root.join(".git/info/exclude"), "/.cortex/\n").unwrap();
        std::fs::write(one.join(".cortex-free.txt"), "x\n").unwrap();
        std::fs::create_dir_all(one.join(".cortex")).unwrap();
        std::fs::write(one.join(".cortex/log"), "x").unwrap();

        assert!(commit_all(&one, "job 1, more").await.unwrap().is_some());
        assert!(run(&one, &["cat-file", "-e", "HEAD:.cortex-free.txt"]));
        assert!(!run(&one, &["cat-file", "-e", "HEAD:.cortex/log"]));
        assert_eq!(commit_all(&two, "again").await.unwrap(), None);

        // Something uncommitted in the project, in a file job 1 does not
        // touch, does not stop the merge.
        std::fs::write(root.join("b.txt"), "mine\n").unwrap();

        let merged = merge(&root, "job-1", "Merge job 1").await.unwrap();

        assert!(matches!(merged, Merged::Done(_)), "{merged:?}");
        assert_eq!(std::fs::read_to_string(root.join("b.txt")).unwrap(), "mine\n");
        assert_eq!(merge(&root, "job-1", "again").await.unwrap(), Merged::AlreadyIn);

        // Job 2 now conflicts, and the project is left alone.
        let before = head(&root).await.unwrap();

        assert_eq!(
            check(&root, "job-2").await.unwrap(),
            Check::Conflicts(vec!["a.txt".into()])
        );
        assert_eq!(
            merge(&root, "job-2", "Merge job 2").await.unwrap(),
            Merged::Conflicts(vec!["a.txt".into()])
        );
        assert_eq!(head(&root).await.unwrap(), before);
        assert!(!in_progress(&root).await);

        // In its own worktree the conflict is there to be resolved.
        let left = merge_into_worktree(&two, &before).await.unwrap();

        assert_eq!(left, vec!["a.txt".to_string()]);
        assert!(in_progress(&two).await);
        assert_eq!(unresolved(&two).await, vec!["a.txt".to_string()]);
        assert!(!in_progress(&root).await);

        // Markers still in the file: nothing is committed.
        assert_eq!(finish_merge(&two).await.unwrap(), vec!["a.txt".to_string()]);
        assert!(in_progress(&two).await);

        std::fs::write(two.join("a.txt"), "one\nTWO and 2\nthree\n").unwrap();

        assert!(finish_merge(&two).await.unwrap().is_empty());
        assert!(!in_progress(&two).await);
        assert_eq!(
            merge(&root, "job-2", "Merge job 2").await.unwrap(),
            Merged::Done(head(&root).await.unwrap())
        );
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "one\nTWO and 2\nthree\n"
        );
    }

    #[test]
    fn markers_are_found_only_as_a_whole_conflict() {
        assert!(has_markers("a\n<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> b\n"));
        assert!(!has_markers("let s = \"<<<<<<< \";\n"));
        assert!(!has_markers("plain\n"));
    }

    #[tokio::test]
    async fn uncommitted_changes_to_incoming_files_hold_the_merge() {
        let Some((_parent, root)) = repository() else {
            return;
        };

        let one = root.with_file_name("one");

        assert!(run(&root, &["worktree", "add", "-q", "-b", "job-1", &one.to_string_lossy()]));

        std::fs::write(one.join("b.txt"), "theirs\n").unwrap();
        commit_all(&one, "job 1").await.unwrap();

        std::fs::write(root.join("b.txt"), "mine\n").unwrap();

        let merged = merge(&root, "job-1", "Merge").await.unwrap();

        assert!(matches!(merged, Merged::Waiting(reason) if reason.contains("b.txt")));
        assert_eq!(std::fs::read_to_string(root.join("b.txt")).unwrap(), "mine\n");

        remove_worktree(&root, &one, Some("job-1"), true).await.unwrap();

        assert!(!one.exists());
        assert!(!run(&root, &["rev-parse", "--verify", "job-1"]));
    }
}
