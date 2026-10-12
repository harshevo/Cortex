//! The symbols of a whole project.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use crate::language::Language;
use crate::symbols::{Symbol, extract_symbols};
use crate::walk;

/// A symbol together with the file it is in.
#[derive(Debug, Clone)]
pub struct Located {
    pub path: PathBuf,
    pub symbol: Symbol,
}

/// Parses files on demand and remembers the result until a file changes.
///
/// The agent edits files while it works, so every lookup compares each
/// file's modification time with the one seen at the last parse.
#[derive(Default)]
pub struct CodeIndex {
    parsed: Mutex<HashMap<PathBuf, ParsedFile>>,
}

struct ParsedFile {
    modified: Option<SystemTime>,
    symbols: Vec<Symbol>,
}

impl CodeIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// The symbols of one file. A file in an unsupported language has none.
    pub fn file_symbols(&self, path: &Path) -> Vec<Symbol> {
        let Some(language) = Language::for_path(path) else {
            return Vec::new();
        };

        let modified = fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .ok();

        let mut parsed = self
            .parsed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if let Some(file) = parsed.get(path)
            && file.modified == modified
        {
            return file.symbols.clone();
        }

        let symbols = match fs::read_to_string(path) {
            Ok(source) => extract_symbols(&source, language),
            Err(_) => Vec::new(),
        };

        parsed.insert(
            path.to_path_buf(),
            ParsedFile {
                modified,
                symbols: symbols.clone(),
            },
        );

        symbols
    }

    /// Every symbol under `root` whose name satisfies `matches`.
    pub fn find(&self, root: &Path, matches: impl Fn(&str) -> bool) -> Vec<Located> {
        let mut found = Vec::new();

        for path in walk::files_under(root) {
            for symbol in self.file_symbols(&path) {
                if matches(&symbol.name) {
                    found.push(Located {
                        path: path.clone(),
                        symbol,
                    });
                }
            }
        }

        found
    }

    /// Symbols named exactly `name`. If there are none, falls back to names
    /// that contain `name`, ignoring case, so a near miss still finds
    /// something.
    pub fn find_by_name(&self, root: &Path, name: &str) -> Vec<Located> {
        let exact = self.find(root, |candidate| candidate == name);

        if !exact.is_empty() {
            return exact;
        }

        let needle = name.to_lowercase();

        self.find(root, |candidate| candidate.to_lowercase().contains(&needle))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_symbols_and_sees_edits() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("money.ts");

        fs::write(&file, "export function toCents() {}\n").unwrap();

        let index = CodeIndex::new();

        let found = index.find_by_name(dir.path(), "toCents");

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, file);

        // A near miss falls back to a case-insensitive substring match.
        assert_eq!(index.find_by_name(dir.path(), "cents").len(), 1);

        // After an edit the old symbol is gone, even though the path was
        // parsed before. The explicit modification time avoids relying on
        // the filesystem's timestamp resolution.
        fs::write(&file, "export function formatMoney() {}\n").unwrap();

        let later = SystemTime::now() + std::time::Duration::from_secs(5);

        fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(later)
            .unwrap();

        assert!(index.find_by_name(dir.path(), "toCents").is_empty());
        assert_eq!(index.find_by_name(dir.path(), "formatMoney").len(), 1);
    }
}
