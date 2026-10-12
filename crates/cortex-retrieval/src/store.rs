//! Keeping a project's vectors on disk between runs.
//!
//! Embedding a project is the slow part of semantic search, and the
//! vectors only change when the code does. They are saved in the project,
//! beside its memory, and read back by the next process. A saved file is
//! trusted only as far as each file's modification time still matches; a
//! file that changed since is embedded again like any other.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::search::IndexedFile;

/// Where the vectors are kept, inside the project.
const FILE: &str = ".cortex/semantic-index.json";

/// Changed whenever chunks or stored vectors change shape, so that a file
/// written by an older build is embedded again instead of misread.
const VERSION: u32 = 1;

/// Which embedding model made the vectors. Those of another model, or of
/// another size, cannot be compared with a query and are thrown away.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Maker {
    pub(crate) model: String,
    pub(crate) dimension: usize,
}

#[derive(Serialize)]
struct Writing<'a> {
    version: u32,
    maker: &'a Maker,

    /// Paths are relative to the project, so that a project that is moved
    /// or copied keeps its vectors.
    files: Vec<(String, &'a IndexedFile)>,
}

#[derive(Deserialize)]
struct Reading {
    version: u32,
    maker: Maker,
    files: Vec<(String, IndexedFile)>,
}

/// The vectors saved for the project at `root`, by absolute path. Empty
/// when there are none that `maker` can use.
pub(crate) fn load(root: &Path, maker: &Maker) -> HashMap<PathBuf, IndexedFile> {
    let Ok(text) = fs::read_to_string(root.join(FILE)) else {
        return HashMap::new();
    };

    let Ok(saved) = serde_json::from_str::<Reading>(&text) else {
        return HashMap::new();
    };

    if saved.version != VERSION || saved.maker != *maker {
        return HashMap::new();
    }

    saved
        .files
        .into_iter()
        .map(|(relative, file)| (root.join(relative), file))
        .collect()
}

/// Saves the vectors of the files under `root`. The file is written
/// beside its place and moved there, so a reader never sees half of it.
pub(crate) fn save(
    root: &Path,
    maker: &Maker,
    files: &HashMap<PathBuf, IndexedFile>,
) -> io::Result<()> {
    let mut under_root: Vec<(String, &IndexedFile)> = files
        .iter()
        .filter_map(|(path, file)| {
            let relative = path.strip_prefix(root).ok()?;

            Some((relative.to_string_lossy().into_owned(), file))
        })
        .collect();

    under_root.sort_by(|left, right| left.0.cmp(&right.0));

    let writing = Writing {
        version: VERSION,
        maker,
        files: under_root,
    };

    let target = root.join(FILE);

    if let Some(directory) = target.parent() {
        fs::create_dir_all(directory)?;
    }

    let partial = target.with_extension("json.partial");

    fs::write(&partial, serde_json::to_vec(&writing)?)?;

    fs::rename(&partial, &target)
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use super::*;
    use crate::chunker::Chunk;
    use crate::quant::Quantized;

    fn maker() -> Maker {
        Maker {
            model: "test-model".to_string(),
            dimension: 3,
        }
    }

    fn indexed(root: &Path) -> HashMap<PathBuf, IndexedFile> {
        let file = IndexedFile {
            modified: Some(SystemTime::UNIX_EPOCH + std::time::Duration::new(1_700_000_000, 123)),
            chunks: vec![Chunk {
                path: "src/money.py".to_string(),
                start_line: 1,
                end_line: 3,
                title: "src/money.py > to_cents".to_string(),
                text: "def to_cents(value):\n    return 1\n".to_string(),
            }],
            vectors: vec![Quantized::from_vector(&[0.5, -0.25, 0.125])],
        };

        HashMap::from([(root.join("src/money.py"), file)])
    }

    #[test]
    fn saved_vectors_are_read_back_as_they_were() {
        let project = tempfile::tempdir().unwrap();
        let files = indexed(project.path());

        save(project.path(), &maker(), &files).unwrap();

        assert_eq!(load(project.path(), &maker()), files);
    }

    #[test]
    fn a_copied_project_keeps_its_vectors() {
        let project = tempfile::tempdir().unwrap();
        let copy = tempfile::tempdir().unwrap();

        save(project.path(), &maker(), &indexed(project.path())).unwrap();

        fs::create_dir_all(copy.path().join(".cortex")).unwrap();
        fs::copy(project.path().join(FILE), copy.path().join(FILE)).unwrap();

        assert_eq!(load(copy.path(), &maker()), indexed(copy.path()));
    }

    #[test]
    fn vectors_of_another_model_are_not_used() {
        let project = tempfile::tempdir().unwrap();

        save(project.path(), &maker(), &indexed(project.path())).unwrap();

        let other = Maker {
            model: "another-model".to_string(),
            dimension: 3,
        };

        assert!(load(project.path(), &other).is_empty());
    }

    #[test]
    fn files_outside_the_project_are_not_saved_with_it() {
        let project = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();

        let mut files = indexed(project.path());

        files.extend(indexed(elsewhere.path()));

        save(project.path(), &maker(), &files).unwrap();

        assert_eq!(load(project.path(), &maker()), indexed(project.path()));
    }

    #[test]
    fn a_damaged_file_counts_as_none() {
        let project = tempfile::tempdir().unwrap();

        fs::create_dir_all(project.path().join(".cortex")).unwrap();
        fs::write(project.path().join(FILE), "{ not json").unwrap();

        assert!(load(project.path(), &maker()).is_empty());
    }
}
