//! Git worktrees for tasks that run side by side.
//!
//! Two sessions editing one checkout overwrite each other. A worktree is
//! a second checkout of the same repository, on a branch of its own, so a
//! task can run there while another runs in the main checkout. Its work
//! comes back the usual way: a merge or a pull request from its branch.
//!
//! Worktrees go beside the project, in `<project>.worktrees/<name>`, so
//! that they are not inside the project's own files and tools.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::Command;

/// A worktree that was made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
}

/// Makes a worktree of the repository at `root` on a new branch
/// `cortex/<name>`, starting from what is checked out now.
pub async fn create(root: &Path, name: &str) -> Result<Worktree, String> {
    let name = clean(name);

    if name.is_empty() {
        return Err("a worktree needs a name of letters, digits, `-` or `_`".to_string());
    }

    let top = git(root, &["rev-parse", "--show-toplevel"])
        .await
        .map_err(|_| "this folder is not in a git repository".to_string())?;

    let top = PathBuf::from(top.trim());

    let folder_name = top
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".to_string());

    let path = top
        .with_file_name(format!("{folder_name}.worktrees"))
        .join(&name);

    if path.exists() {
        return Err(format!("{} already exists", path.display()));
    }

    let branch = format!("cortex/{name}");

    git(
        &top,
        &[
            "worktree",
            "add",
            "-b",
            &branch,
            &path.to_string_lossy(),
            "HEAD",
        ],
    )
    .await?;

    Ok(Worktree { path, branch })
}

/// The worktrees of the repository at `root`, the main checkout first.
pub async fn list(root: &Path) -> Result<Vec<Worktree>, String> {
    let listed = git(root, &["worktree", "list", "--porcelain"]).await?;

    let mut found = Vec::new();
    let mut path = None;

    for line in listed.lines() {
        if let Some(rest) = line.strip_prefix("worktree ") {
            path = Some(PathBuf::from(rest));
        } else if let Some(rest) = line.strip_prefix("branch ") {
            if let Some(path) = path.take() {
                found.push(Worktree {
                    path,
                    branch: rest.trim_start_matches("refs/heads/").to_string(),
                });
            }
        } else if line == "detached"
            && let Some(path) = path.take()
        {
            found.push(Worktree {
                path,
                branch: "(detached)".to_string(),
            });
        }
    }

    Ok(found)
}

fn clean(name: &str) -> String {
    name.trim()
        .chars()
        .map(|c| match c {
            c if c.is_ascii_alphanumeric() || c == '-' || c == '_' => c,
            _ => '-',
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn run(root: &Path, args: &[&str]) -> bool {
        std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .is_ok_and(|output| output.status.success())
    }

    #[tokio::test]
    async fn a_worktree_is_made_beside_the_project_on_its_own_branch() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("app");
        std::fs::create_dir_all(&root).unwrap();

        let ready = run(&root, &["init", "--quiet"])
            && run(
                &root,
                &[
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "commit",
                    "--allow-empty",
                    "-qm",
                    "start",
                ],
            );

        if !ready {
            return;
        }

        let made = create(&root, "fix login!").await.unwrap();

        assert_eq!(made.branch, "cortex/fix-login");
        assert!(made.path.ends_with("app.worktrees/fix-login"));
        assert!(made.path.is_dir());

        let all = list(&root).await.unwrap();

        assert_eq!(all.len(), 2);
        assert!(
            all.iter()
                .any(|worktree| worktree.branch == "cortex/fix-login")
        );

        assert!(create(&root, "fix login!").await.is_err());
    }
}
