//! Cutting source files into pieces worth embedding.
//!
//! A chunk should be one coherent unit of code: small enough that its
//! vector is about one thing, large enough to be useful when returned.
//! Where the code index knows the file's symbols, each function, method or
//! type is a chunk. Other files are cut into fixed windows of lines.

use std::path::Path;

use cortex_index::{CodeIndex, Symbol, SymbolKind};
use serde::{Deserialize, Serialize};

/// Symbols longer than this are cut into windows.
const MAX_CHUNK_LINES: usize = 80;

/// Window size and overlap for code that is not split along symbols.
const WINDOW_LINES: usize = 60;
const WINDOW_OVERLAP: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chunk {
    /// File path relative to the project directory.
    pub path: String,

    /// First and last line, counted from 1.
    pub start_line: usize,
    pub end_line: usize,

    /// What the chunk is, for the embedding model and for display:
    /// `src/lib/money.ts > toCents`.
    pub title: String,

    pub text: String,
}

/// Splits one file. `relative_path` is used for titles only.
pub fn chunk_file(index: &CodeIndex, path: &Path, relative_path: &str, source: &str) -> Vec<Chunk> {
    let lines: Vec<&str> = source.lines().collect();

    if lines.is_empty() {
        return Vec::new();
    }

    let symbols = index.file_symbols(path);

    let units: Vec<&Symbol> = symbols
        .iter()
        .filter(|symbol| is_unit(symbol, &symbols))
        .collect();

    if units.is_empty() {
        return windows(relative_path, relative_path, &lines, 1, lines.len());
    }

    let mut chunks = Vec::new();

    for symbol in units {
        let title = title_for(relative_path, symbol);

        let length = symbol.end_line - symbol.start_line + 1;

        if length <= MAX_CHUNK_LINES {
            chunks.push(chunk(
                relative_path,
                &title,
                &lines,
                symbol.start_line,
                symbol.end_line,
            ));
        } else {
            chunks.extend(windows(
                relative_path,
                &title,
                &lines,
                symbol.start_line,
                symbol.end_line,
            ));
        }
    }

    chunks
}

/// Whether a symbol should become a chunk of its own.
///
/// A class or module that contains other symbols is left out, because its
/// methods are chunked individually and would otherwise be embedded twice.
fn is_unit(symbol: &Symbol, all: &[Symbol]) -> bool {
    let has_members = all.iter().any(|other| {
        other.depth == symbol.depth + 1
            && other.start_line >= symbol.start_line
            && other.end_line <= symbol.end_line
            && other.container.as_deref() == Some(symbol.name.as_str())
    });

    let is_container = matches!(
        symbol.kind,
        SymbolKind::Class
            | SymbolKind::Interface
            | SymbolKind::Trait
            | SymbolKind::Impl
            | SymbolKind::Module
    );

    !(is_container && has_members)
}

fn title_for(relative_path: &str, symbol: &Symbol) -> String {
    match &symbol.container {
        Some(container) => format!("{relative_path} > {container}.{}", symbol.name),
        None => format!("{relative_path} > {}", symbol.name),
    }
}

/// Overlapping windows covering lines `first..=last`.
fn windows(path: &str, title: &str, lines: &[&str], first: usize, last: usize) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let mut start = first;

    loop {
        let end = (start + WINDOW_LINES - 1).min(last);

        chunks.push(chunk(path, title, lines, start, end));

        if end == last {
            break;
        }

        start = end + 1 - WINDOW_OVERLAP;
    }

    chunks
}

fn chunk(path: &str, title: &str, lines: &[&str], first: usize, last: usize) -> Chunk {
    Chunk {
        path: path.to_string(),
        start_line: first,
        end_line: last,
        title: title.to_string(),
        text: lines[first - 1..last].join("\n"),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn chunks_of(file_name: &str, source: &str) -> Vec<Chunk> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(file_name);

        fs::write(&path, source).unwrap();

        chunk_file(&CodeIndex::new(), &path, file_name, source)
    }

    #[test]
    fn each_function_and_method_is_a_chunk() {
        let source = "\
def top():
    return 1

class Cart:
    def total(self):
        return 0

    def add(self, item):
        pass
";

        let chunks = chunks_of("shop.py", source);

        let titles: Vec<&str> = chunks.iter().map(|chunk| chunk.title.as_str()).collect();

        // The class itself is not a chunk; its methods are.
        assert_eq!(
            titles,
            vec![
                "shop.py > top",
                "shop.py > Cart.total",
                "shop.py > Cart.add"
            ]
        );

        assert_eq!((chunks[1].start_line, chunks[1].end_line), (5, 6));
        assert!(chunks[1].text.contains("def total(self):"));
    }

    #[test]
    fn files_without_symbols_are_cut_into_overlapping_windows() {
        let source: String = (1..=130).map(|line| format!("line {line}\n")).collect();

        let chunks = chunks_of("notes.md", &source);

        let ranges: Vec<(usize, usize)> = chunks
            .iter()
            .map(|chunk| (chunk.start_line, chunk.end_line))
            .collect();

        assert_eq!(ranges, vec![(1, 60), (51, 110), (101, 130)]);
    }

    #[test]
    fn long_functions_are_split() {
        let body: String = (0..120).map(|line| format!("    x = {line}\n")).collect();
        let source = format!("def long():\n{body}");

        let chunks = chunks_of("long.py", &source);

        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|chunk| chunk.title == "long.py > long"));
        assert_eq!(chunks.last().unwrap().end_line, 121);
    }
}
