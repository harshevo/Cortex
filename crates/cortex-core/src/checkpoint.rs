//! Snapshots of the project that a bad run can be undone to.
//!
//! Before a run first changes anything, the state of the project is
//! saved. Restoring a checkpoint puts every file back as it was then and
//! removes the files made since, whether a file tool or a shell command
//! made them.
//!
//! The snapshots are commits in a git repository of their own, kept in
//! `.cortex/checkpoints` inside the project. It shares nothing with the
//! project's own repository, if it has one: the project's branches, index
//! and history are never touched, and a project without git gets
//! checkpoints all the same. What the project's `.gitignore` leaves out is
//! left out of the snapshots too, along with build output that is
//! commonly not ignored.
//!
//! Git has to be installed. Without it there are no checkpoints, and
//! saving one says so instead of failing the run.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::Command;

/// Where the snapshots are kept, relative to the project.
pub const FOLDER: &str = ".cortex/checkpoints";

/// Never part of a snapshot, whatever the project ignores.
const EXCLUDED: &str = "\
/.git/
/.cortex/
node_modules/
target/
.venv/
__pycache__/
.DS_Store
";

/// One saved state of the project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    /// The commit that holds it.
    pub id: String,

    /// Seconds since 1970, UTC.
    pub time: u64,

    /// What it was saved before.
    pub label: String,
}

impl Checkpoint {
    /// The first characters of the id, enough to name it.
    pub fn short(&self) -> &str {
        &self.id[..self.id.len().min(8)]
    }
}

/// What a restore did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Restored {
    /// The checkpoint the project was put back to.
    pub to: String,

    /// Files whose content was put back.
    pub reverted: Vec<String>,

    /// Files made after the checkpoint, now removed.
    pub removed: Vec<String>,

    /// The state just before the restore, so the restore itself can be
    /// undone. `None` when nothing had changed.
    pub before: Option<Checkpoint>,
}

impl Restored {
    pub fn changed(&self) -> usize {
        self.reverted.len() + self.removed.len()
    }
}

/// The checkpoints of one project.
#[derive(Debug, Clone)]
pub struct Checkpoints {
    root: PathBuf,
    store: PathBuf,
}

impl Checkpoints {
    pub fn for_project(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref().to_path_buf();
        let store = root.join(FOLDER);

        Self { root, store }
    }

    /// Saves the project as it is now. When nothing changed since the
    /// last checkpoint, that one is returned instead of a new one.
    pub async fn save(&self, label: &str) -> Result<Checkpoint, String> {
        self.ensure_store().await?;

        self.git(&["add", "--all"]).await?;

        let label = one_line(label);

        let committed = self
            .git(&["commit", "--quiet", "--no-verify", "-m", &label])
            .await;

        if let Err(error) = committed {
            // Nothing new to save: the latest checkpoint is this state.
            if let Some(latest) = self.list(1).await?.into_iter().next()
                && self.unchanged_since(&latest.id).await
            {
                return Ok(latest);
            }

            return Err(error);
        }

        self.list(1)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| "the checkpoint was not recorded".to_string())
    }

    /// The most recent checkpoints, newest first.
    pub async fn list(&self, limit: usize) -> Result<Vec<Checkpoint>, String> {
        if !self.exists() {
            return Ok(Vec::new());
        }

        let limit = format!("--max-count={limit}");

        let Ok(log) = self.git(&["log", &limit, "--format=%H%x1f%ct%x1f%s"]).await else {
            // A store with no commit yet has no log.
            return Ok(Vec::new());
        };

        Ok(log.lines().filter_map(parse_entry).collect())
    }

    /// The files that differ between `id` and the project as it is now,
    /// as `git diff --stat` writes it.
    pub async fn diff_stat(&self, id: &str) -> Result<String, String> {
        self.ensure_store().await?;
        self.git(&["add", "--all"]).await?;

        self.git(&["diff", "--cached", "--stat", "--no-renames", id])
            .await
    }

    /// Puts the project back as it was at `id`.
    ///
    /// The present state is saved first, so a restore can itself be
    /// undone.
    pub async fn restore(&self, id: &str) -> Result<Restored, String> {
        let target = self
            .git(&["rev-parse", "--verify", &format!("{id}^{{commit}}")])
            .await
            .map_err(|_| format!("there is no checkpoint `{id}`"))?;

        let target = target.trim().to_string();

        let before = self.save("before restoring a checkpoint").await?;

        let changes = self
            .git(&[
                "diff",
                "--name-status",
                "--no-renames",
                "-z",
                &target,
                &before.id,
            ])
            .await?;

        let mut restored = Restored {
            to: target.clone(),
            before: (before.id != target).then_some(before),
            ..Restored::default()
        };

        let mut fields = changes.split('\0').filter(|field| !field.is_empty());

        while let (Some(status), Some(path)) = (fields.next(), fields.next()) {
            if status.starts_with('A') {
                // Made after the checkpoint.
                let file = self.root.join(path);

                if std::fs::remove_file(&file).is_ok() {
                    remove_empty_parents(&file, &self.root);
                }

                restored.removed.push(path.to_string());
            } else {
                restored.reverted.push(path.to_string());
            }
        }

        // In batches, so that a large restore stays within the limit on
        // the length of a command line.
        for batch in restored.reverted.chunks(200) {
            let mut args = vec!["checkout", target.as_str(), "--"];

            args.extend(batch.iter().map(String::as_str));

            self.git(&args).await?;
        }

        // The store's index follows the files, so the next save sees
        // exactly what changed after the restore.
        self.git(&["read-tree", &target]).await?;

        Ok(restored)
    }

    fn exists(&self) -> bool {
        self.store.join("HEAD").is_file()
    }

    async fn ensure_store(&self) -> Result<(), String> {
        if self.exists() {
            return Ok(());
        }

        std::fs::create_dir_all(&self.store)
            .map_err(|error| format!("could not create {}: {error}", self.store.display()))?;

        let output = Command::new("git")
            .args(["init", "--quiet", "--bare"])
            .arg(&self.store)
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(|error| format!("git is needed for checkpoints: {error}"))?;

        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
        }

        let info = self.store.join("info");

        std::fs::create_dir_all(&info)
            .and_then(|()| std::fs::write(info.join("exclude"), EXCLUDED))
            .map_err(|error| format!("could not write the checkpoint exclusions: {error}"))?;

        // A bare repository has no work tree of its own; this one is the
        // project's.
        self.git(&["config", "core.bare", "false"]).await?;

        Ok(())
    }

    async fn unchanged_since(&self, id: &str) -> bool {
        self.git(&["diff", "--cached", "--quiet", id]).await.is_ok()
    }

    /// Runs git on the store, with the project as its work tree. Returns
    /// standard output, or standard error when git fails.
    async fn git(&self, args: &[&str]) -> Result<String, String> {
        let output = Command::new("git")
            .arg("--git-dir")
            .arg(&self.store)
            .arg("--work-tree")
            .arg(&self.root)
            .args([
                "-c",
                "user.name=Cortex",
                "-c",
                "user.email=checkpoints@cortex.local",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.autocrlf=false",
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "advice.addIgnoredFile=false",
            ])
            .args(args)
            .current_dir(&self.root)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(|error| format!("git is needed for checkpoints: {error}"))?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            let errors = String::from_utf8_lossy(&output.stderr).trim().to_string();

            Err(match errors.is_empty() {
                true => String::from_utf8_lossy(&output.stdout).trim().to_string(),
                false => errors,
            })
        }
    }
}

fn parse_entry(line: &str) -> Option<Checkpoint> {
    let mut parts = line.split('\x1f');

    Some(Checkpoint {
        id: parts.next()?.to_string(),
        time: parts.next()?.parse().ok()?,
        label: parts.next().unwrap_or_default().to_string(),
    })
}

/// A label fit for a commit subject.
fn one_line(text: &str) -> String {
    let line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("checkpoint");

    let mut line: String = line.trim().chars().take(100).collect();

    if line.is_empty() {
        line.push_str("checkpoint");
    }

    line
}

/// Removes the folders a removed file leaves empty, up to the root.
fn remove_empty_parents(file: &Path, root: &Path) {
    let mut folder = file.parent();

    while let Some(current) = folder {
        if current == root || !current.starts_with(root) {
            break;
        }

        if std::fs::remove_dir(current).is_err() {
            break;
        }

        folder = current.parent();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git_installed() -> bool {
        std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
    }

    #[tokio::test]
    async fn restoring_puts_files_back_and_removes_new_ones() {
        if !git_installed() {
            return;
        }

        let project = tempfile::tempdir().unwrap();
        let root = project.path();

        std::fs::write(root.join("kept.txt"), "original").unwrap();
        std::fs::write(root.join("deleted.txt"), "still here").unwrap();

        let checkpoints = Checkpoints::for_project(root);

        let saved = checkpoints.save("before the task").await.unwrap();

        assert_eq!(saved.label, "before the task");

        std::fs::write(root.join("kept.txt"), "changed").unwrap();
        std::fs::remove_file(root.join("deleted.txt")).unwrap();
        std::fs::create_dir_all(root.join("new/deep")).unwrap();
        std::fs::write(root.join("new/deep/made.txt"), "new").unwrap();

        let restored = checkpoints.restore(&saved.id).await.unwrap();

        assert_eq!(
            std::fs::read_to_string(root.join("kept.txt")).unwrap(),
            "original"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("deleted.txt")).unwrap(),
            "still here"
        );
        assert!(!root.join("new").exists());
        assert_eq!(restored.removed, vec!["new/deep/made.txt".to_string()]);
        assert_eq!(restored.changed(), 3);

        // The restore can itself be undone.
        let before = restored.before.unwrap();

        checkpoints.restore(&before.id).await.unwrap();

        assert_eq!(
            std::fs::read_to_string(root.join("kept.txt")).unwrap(),
            "changed"
        );
        assert!(root.join("new/deep/made.txt").exists());
    }

    #[tokio::test]
    async fn saving_an_unchanged_project_reuses_the_last_checkpoint() {
        if !git_installed() {
            return;
        }

        let project = tempfile::tempdir().unwrap();

        std::fs::write(project.path().join("a.txt"), "a").unwrap();

        let checkpoints = Checkpoints::for_project(project.path());

        let first = checkpoints.save("one").await.unwrap();
        let second = checkpoints.save("two").await.unwrap();

        assert_eq!(first.id, second.id);
        assert_eq!(checkpoints.list(10).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn ignored_files_and_the_store_are_left_out() {
        if !git_installed() {
            return;
        }

        let project = tempfile::tempdir().unwrap();
        let root = project.path();

        std::fs::write(root.join(".gitignore"), "secret.env\n").unwrap();
        std::fs::write(root.join("secret.env"), "KEY=1").unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::write(root.join("target/out.bin"), "x").unwrap();

        let checkpoints = Checkpoints::for_project(root);
        let saved = checkpoints.save("start").await.unwrap();

        let files = checkpoints
            .git(&["ls-tree", "-r", "--name-only", &saved.id])
            .await
            .unwrap();

        assert_eq!(files.trim(), ".gitignore");
    }
}
