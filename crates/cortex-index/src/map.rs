//! A one-screen map of a project: its source files and what each defines.
//!
//! In the measured baseline about half the steps of a run went on finding
//! the code to change. The map gives the model the layout before its first
//! step, so it can open the right file instead of listing and searching
//! its way there.
//!
//! The map has a size limit. A project that does not fit is shown in less
//! detail rather than cut short: fewer names per file, then paths only,
//! then directories with file counts.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::index::CodeIndex;
use crate::language::Language;
use crate::walk;

/// Above this many source files, parsing all of them before a run starts
/// takes too long, so only paths are listed.
const MAX_PARSED_FILES: usize = 2_000;

/// Names listed for one file at each level of detail.
const NAMES_PER_FILE: [usize; 2] = [12, 5];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoMap {
    pub text: String,

    /// Source files in the project.
    pub files: usize,

    /// How the project was drawn: `symbols`, `fewer_symbols`, `paths` or
    /// `directories`.
    pub detail: &'static str,
}

struct SourceFile {
    path: String,
    lines: usize,

    /// Names defined at the top level of the file, in order.
    names: Vec<String>,

    is_test: bool,
}

/// Draws the project under `root` in at most `max_chars` characters.
/// `None` when the project has no source file the index understands.
pub fn repo_map(index: &CodeIndex, root: &Path, max_chars: usize) -> Option<RepoMap> {
    let files = source_files(index, root);

    if files.is_empty() {
        return None;
    }

    let drawings = [
        ("symbols", with_names(&files, NAMES_PER_FILE[0])),
        ("fewer_symbols", with_names(&files, NAMES_PER_FILE[1])),
        ("paths", paths_only(&files)),
        ("directories", directories(&files)),
    ];

    let last = drawings.len() - 1;

    for (position, (detail, text)) in drawings.into_iter().enumerate() {
        if text.len() <= max_chars || position == last {
            return Some(RepoMap {
                text: cut_at_line(&text, max_chars),
                files: files.len(),
                detail,
            });
        }
    }

    None
}

fn source_files(index: &CodeIndex, root: &Path) -> Vec<SourceFile> {
    let paths: Vec<_> = walk::files_under(root)
        .into_iter()
        .filter(|path| Language::for_path(path).is_some())
        .collect();

    let parse = paths.len() <= MAX_PARSED_FILES;

    paths
        .iter()
        .map(|path| {
            let relative = path
                .strip_prefix(root)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/");

            let names = if parse {
                top_level_names(index, path)
            } else {
                Vec::new()
            };

            SourceFile {
                lines: line_count(path),
                names,
                is_test: walk::is_test_file(&relative),
                path: relative,
            }
        })
        .collect()
}

fn top_level_names(index: &CodeIndex, path: &Path) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();

    for symbol in index.file_symbols(path) {
        if symbol.depth == 0 && !names.contains(&symbol.name) {
            names.push(symbol.name);
        }
    }

    names
}

fn line_count(path: &Path) -> usize {
    fs::read_to_string(path)
        .map(|source| source.lines().count())
        .unwrap_or_default()
}

/// One line per file: `src/fees.ts (120): computeFee, FeeSchedule`.
/// Test files are listed without names; their names say little.
fn with_names(files: &[SourceFile], limit: usize) -> String {
    let mut text = String::new();

    for file in files {
        text.push_str(&format!("{} ({})", file.path, file.lines));

        if !file.is_test && !file.names.is_empty() {
            let shown: Vec<&str> = file.names.iter().take(limit).map(String::as_str).collect();

            text.push_str(": ");
            text.push_str(&shown.join(", "));

            if file.names.len() > limit {
                text.push_str(&format!(", +{} more", file.names.len() - limit));
            }
        }

        text.push('\n');
    }

    text
}

fn paths_only(files: &[SourceFile]) -> String {
    let mut text = String::new();

    for file in files {
        text.push_str(&format!("{} ({})\n", file.path, file.lines));
    }

    text
}

/// One line per directory: `src/lib/  14 files`.
fn directories(files: &[SourceFile]) -> String {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();

    for file in files {
        let directory = file
            .path
            .rsplit_once('/')
            .map_or(".", |(directory, _)| directory);

        *counts.entry(directory).or_default() += 1;
    }

    let mut text = String::new();

    for (directory, count) in counts {
        text.push_str(&format!("{directory}/  {count} files\n"));
    }

    text
}

/// Keeps whole lines up to `max_chars` and says how many were left out.
fn cut_at_line(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }

    let mut kept = String::new();
    let mut shown = 0;

    for line in text.lines() {
        if kept.len() + line.len() + 1 > max_chars {
            break;
        }

        kept.push_str(line);
        kept.push('\n');

        shown += 1;
    }

    let left_out = text.lines().count() - shown;

    kept.push_str(&format!("[{left_out} more lines not shown]\n"));

    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();

        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::create_dir_all(dir.path().join("tests")).unwrap();

        fs::write(
            dir.path().join("src/money.ts"),
            "export function toCents(value: string) {\n  return 1;\n}\n\n\
             export class Wallet {\n  balance() {\n    return 0;\n  }\n}\n",
        )
        .unwrap();

        fs::write(
            dir.path().join("tests/money.test.ts"),
            "function helper() {\n  return 1;\n}\n",
        )
        .unwrap();

        fs::write(dir.path().join("README.md"), "# Notes\n").unwrap();

        dir
    }

    #[test]
    fn files_are_listed_with_their_top_level_names() {
        let dir = project();

        let map = repo_map(&CodeIndex::new(), dir.path(), 4_000).unwrap();

        assert_eq!(map.detail, "symbols");
        assert_eq!(map.files, 2);

        // Methods are left out, and so are files that are not source code.
        assert!(map.text.contains("src/money.ts (9): toCents, Wallet\n"));
        assert!(!map.text.contains("balance"));
        assert!(!map.text.contains("README"));

        // A test file is listed, without its names.
        assert!(map.text.contains("tests/money.test.ts (3)\n"));
    }

    #[test]
    fn a_project_that_does_not_fit_is_drawn_in_less_detail() {
        let dir = project();

        let map = repo_map(&CodeIndex::new(), dir.path(), 50).unwrap();

        assert_eq!(map.detail, "paths");
        assert!(map.text.len() <= 50);
        assert!(!map.text.contains("toCents"));

        let map = repo_map(&CodeIndex::new(), dir.path(), 30).unwrap();

        assert_eq!(map.detail, "directories");
        assert_eq!(map.text, "src/  1 files\ntests/  1 files\n");
    }

    #[test]
    fn a_project_without_source_files_has_no_map() {
        let dir = tempfile::tempdir().unwrap();

        fs::write(dir.path().join("README.md"), "# Notes\n").unwrap();

        assert!(repo_map(&CodeIndex::new(), dir.path(), 4_000).is_none());
    }
}
