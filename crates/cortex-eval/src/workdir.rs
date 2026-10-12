//! Throwaway working copies of a task's project.

use std::fs;
use std::io;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use tempfile::TempDir;

use crate::task::{SourceSpec, Task};

/// A fresh working copy of the task's project in a temporary directory. The
/// directory is deleted when the returned handle is dropped.
pub fn prepare(task: &Task) -> Result<TempDir> {
    let workdir = tempfile::Builder::new()
        .prefix(&format!("cortex-eval-{}-", task.id))
        .tempdir()
        .context("could not create a temporary directory")?;

    if let (Some(source), Some(repo), Some(link_dir)) =
        (&task.source, task.source_repo(), task.link_dir())
    {
        export_source(source, &repo, &link_dir, workdir.path())
            .with_context(|| format!("could not prepare the source for task `{}`", task.id))?;
    }

    // With a source, `repo/` holds only the files the task changes.
    copy_tree(&task.repo_dir(), workdir.path())
        .with_context(|| format!("could not copy the repo for task `{}`", task.id))?;

    Ok(workdir)
}

/// Builds the starting project from a git repository.
fn export_source(source: &SourceSpec, repo: &Path, link_dir: &Path, workdir: &Path) -> Result<()> {
    let repo = repo
        .canonicalize()
        .with_context(|| format!("the source repository {} does not exist", repo.display()))?;

    export_commit(&repo, &source.commit, workdir)?;

    for path in &source.remove {
        remove_path(&workdir.join(path))
            .with_context(|| format!("could not remove `{path}` from the working copy"))?;
    }

    for path in &source.link {
        let target = link_dir.join(path);

        if !target.exists() {
            bail!(
                "`{path}` is missing from {}; install the project's dependencies there first",
                link_dir.display()
            );
        }

        let target = target.canonicalize()?;

        symlink(&target, &workdir.join(path))
            .with_context(|| format!("could not link `{path}` into the working copy"))?;
    }

    Ok(())
}

/// Writes the files of `commit` into `workdir`, without the `.git` history.
fn export_commit(repo: &Path, commit: &str, workdir: &Path) -> Result<()> {
    // `$1`..`$3` keep the paths out of the shell's own parsing.
    let status = Command::new("sh")
        .arg("-c")
        .arg(r#"git -C "$1" archive --format=tar "$2" | tar -x -C "$3""#)
        .arg("export")
        .arg(repo)
        .arg(commit)
        .arg(workdir)
        .status()
        .context("could not run git")?;

    let is_empty = fs::read_dir(workdir)?.next().is_none();

    if !status.success() || is_empty {
        bail!("could not export commit `{commit}` from {}", repo.display());
    }

    Ok(())
}

fn remove_path(path: &Path) -> io::Result<()> {
    if path.is_dir() {
        fs::remove_dir_all(path)
    } else if path.exists() {
        fs::remove_file(path)
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(not(unix))]
fn symlink(_target: &Path, _link: &Path) -> io::Result<()> {
    Err(io::Error::other(
        "linked paths are only supported on Unix systems",
    ))
}

/// Copies `from` into `to`, overwriting files that already exist. A missing
/// source directory is treated as empty.
pub fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    if !from.is_dir() {
        return Ok(());
    }

    fs::create_dir_all(to)?;

    for entry in fs::read_dir(from)? {
        let entry = entry?;

        let source = entry.path();
        let target = to.join(entry.file_name());

        if entry.file_type()?.is_dir() {
            copy_tree(&source, &target)?;
        } else {
            fs::copy(&source, &target)?;
        }
    }

    Ok(())
}
