//! Walking the project tree the same way for every caller.

use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

/// Directories that are never worth showing to the model.
const SKIPPED_DIRECTORIES: [&str; 2] = [".git", ".cortex"];

/// Directories of installed dependencies and build output. A project
/// usually lists them in `.gitignore`; one without a `.gitignore` (a
/// folder the agent has just started a project in) would otherwise have
/// all of them read, indexed and embedded.
const DEPENDENCY_DIRECTORIES: [&str; 9] = [
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    ".venv",
    "venv",
    "__pycache__",
    ".gradle",
];

/// Every file under `start`, sorted, honouring `.gitignore` and leaving
/// out installed dependencies and build output.
///
/// Hidden files are included because projects keep real configuration in
/// them. Symbolic links are not followed.
pub fn files_under(start: &Path) -> Vec<PathBuf> {
    let walker = WalkBuilder::new(start)
        .hidden(false)
        .require_git(false)
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();

            let is_directory = entry.file_type().is_some_and(|kind| kind.is_dir());

            // `start` itself is always walked, whatever it is called.
            let is_dependencies = is_directory
                && entry.depth() > 0
                && DEPENDENCY_DIRECTORIES.contains(&name.as_ref());

            !SKIPPED_DIRECTORIES.contains(&name.as_ref()) && !is_dependencies
        })
        .build();

    let mut files: Vec<PathBuf> = walker
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .map(|entry| entry.into_path())
        .collect();

    files.sort();

    files
}

/// Whether a path, relative to the project, is a test file by the usual
/// conventions: a `tests` directory, `x.test.ts`, `test_x.py`, `x_test.rs`.
pub fn is_test_file(relative_path: &str) -> bool {
    let lowered = relative_path.to_lowercase();

    lowered.split(['/', '\\']).any(|part| {
        matches!(part, "test" | "tests" | "__tests__" | "spec")
            || part.contains(".test.")
            || part.contains(".spec.")
            || part.starts_with("test_")
            || part.ends_with("_test.py")
            || part.ends_with("_test.rs")
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn installed_dependencies_are_left_out_without_a_gitignore() {
        let dir = tempfile::tempdir().unwrap();

        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::create_dir_all(dir.path().join("node_modules/react")).unwrap();

        fs::write(dir.path().join("src/App.jsx"), "").unwrap();
        fs::write(dir.path().join("node_modules/react/index.js"), "").unwrap();

        // A file, not a directory, so its name does not matter.
        fs::write(dir.path().join("build"), "").unwrap();

        let files: Vec<String> = files_under(dir.path())
            .iter()
            .map(|path| path.strip_prefix(dir.path()).unwrap().display().to_string())
            .collect();

        assert_eq!(files, ["build", "src/App.jsx"]);
    }
}
