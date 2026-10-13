//! The directory an agent is allowed to work in.

use std::io;
use std::path::{Component, Path, PathBuf};

use tokio::process::Command;

use crate::error::ToolError;
use crate::sandbox::Sandbox;

/// A project root. File tools resolve every path through it so they cannot
/// read or write outside the project, and the commands tools start are
/// held to it by the [`Sandbox`].
#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
    sandbox: Sandbox,
}

impl Workspace {
    pub fn new(root: impl AsRef<Path>) -> io::Result<Self> {
        let root = root.as_ref().canonicalize()?;

        Ok(Self {
            root,
            sandbox: Sandbox::default(),
        })
    }

    /// The sandbox is on unless this switches it off.
    pub fn with_sandbox(mut self, sandbox: Sandbox) -> Self {
        self.sandbox = sandbox;

        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn sandbox(&self) -> Sandbox {
        self.sandbox
    }

    /// A command that runs `program` for a tool: confined to the project
    /// when the sandbox is on. The caller still chooses its directory.
    pub fn command(&self, program: &str) -> Command {
        self.sandbox.command(program, &self.root)
    }

    /// Turns a model-supplied path into an absolute path inside the root.
    pub fn resolve(&self, path: &str) -> Result<PathBuf, ToolError> {
        let candidate = Path::new(path);

        let joined = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            self.root.join(candidate)
        };

        let normalized = normalize(&joined);

        if !normalized.starts_with(&self.root) {
            return Err(ToolError::InvalidInput(format!(
                "path `{path}` is outside the workspace"
            )));
        }

        self.reject_symlink_escape(&normalized, path)?;

        Ok(normalized)
    }

    /// Path relative to the root, for display.
    pub fn relative(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .display()
            .to_string()
    }

    /// A lexically valid path can still leave the root through a symlink.
    /// Check the real location of the deepest part that already exists.
    fn reject_symlink_escape(&self, normalized: &Path, original: &str) -> Result<(), ToolError> {
        let existing = normalized
            .ancestors()
            .find(|ancestor| ancestor.exists())
            .unwrap_or(&self.root);

        let real = existing
            .canonicalize()
            .map_err(|error| ToolError::Failed(format!("cannot resolve `{original}`: {error}")))?;

        if !real.starts_with(&self.root) {
            return Err(ToolError::InvalidInput(format!(
                "path `{original}` leads outside the workspace"
            )));
        }

        Ok(())
    }
}

/// Removes `.` and `..` without touching the filesystem.
fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::CurDir => {}

            Component::ParentDir => {
                normalized.pop();
            }

            other => normalized.push(other.as_os_str()),
        }
    }

    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_relative_paths_inside_the_root() {
        let dir = std::env::temp_dir();
        let workspace = Workspace::new(&dir).unwrap();

        let resolved = workspace.resolve("src/./main.rs").unwrap();

        assert_eq!(resolved, workspace.root().join("src/main.rs"));
    }

    #[test]
    fn rejects_paths_that_climb_out() {
        let dir = std::env::temp_dir();
        let workspace = Workspace::new(&dir).unwrap();

        assert!(workspace.resolve("../../etc/passwd").is_err());
        assert!(workspace.resolve("/etc/passwd").is_err());
    }
}
