//! The vectors of a project, and ranking them against a query.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use cortex_index::{CodeIndex, Language, walk};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, OnceCell};

use crate::chunker::{self, Chunk};
use crate::client::{EmbeddingClient, EmbeddingError};
use crate::quant::{self, Quantized};
use crate::store::{self, Maker};

/// Larger files are generated or data, not code worth searching by meaning.
const MAX_FILE_BYTES: u64 = 200_000;

/// Up to this many chunks, every one is compared in full. Beyond it, a
/// scan of the sign bits picks the ones worth comparing.
const SCAN_ALL_BELOW: usize = 4_000;

/// Chunks compared in full for each result asked for, in a large project.
const SHORTLIST_PER_RESULT: usize = 40;

#[derive(Debug, Clone)]
pub struct Hit {
    pub chunk: Chunk,

    /// Cosine similarity between the query and the chunk, from -1 to 1.
    pub score: f32,
}

/// How much of a project is embedded.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IndexSize {
    pub files: usize,
    pub chunks: usize,

    /// Memory held by the vectors.
    pub bytes: usize,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct IndexedFile {
    pub(crate) modified: Option<SystemTime>,
    pub(crate) chunks: Vec<Chunk>,

    /// Kept in their small forms; see [`crate::quant`].
    pub(crate) vectors: Vec<Quantized>,
}

/// The vectors in memory, and which projects' saved vectors are among
/// them.
#[derive(Default)]
struct Indexed {
    files: HashMap<PathBuf, IndexedFile>,

    /// Projects whose saved vectors have been read, or looked for.
    read_from_disk: HashSet<PathBuf>,
}

/// Embeds a project's code on first use and keeps it up to date.
///
/// The agent edits files between searches, so every search first re-embeds
/// the files whose modification time changed. The vectors are saved in
/// the project (see [`crate::store`]), so a fresh process embeds only what
/// changed since the last one.
pub struct SemanticIndex {
    client: EmbeddingClient,
    code_index: Arc<CodeIndex>,
    files: Mutex<Indexed>,

    /// The model behind the embedding service, asked once. `None` when it
    /// could not be asked: nothing is read from or saved to disk then.
    maker: OnceCell<Option<Maker>>,
}

impl SemanticIndex {
    pub fn new(client: EmbeddingClient, code_index: Arc<CodeIndex>) -> Self {
        Self {
            client,
            code_index,
            files: Mutex::new(Indexed::default()),
            maker: OnceCell::new(),
        }
    }

    /// The `limit` chunks under `root` most similar to `query`, best first.
    pub async fn search(
        &self,
        root: &Path,
        query: &str,
        limit: usize,
    ) -> Result<Vec<Hit>, EmbeddingError> {
        // One search at a time: holding the lock across the refresh keeps
        // two searches from embedding the same files twice.
        let mut files = self.files.lock().await;

        self.search_in(root, query, limit, &mut files).await
    }

    /// [`Self::search`], unless the index is in use: being warmed, or
    /// answering another search. `None` then, at once, for a caller that
    /// would rather go without than wait for a project to be embedded.
    pub async fn search_unless_busy(
        &self,
        root: &Path,
        query: &str,
        limit: usize,
    ) -> Result<Option<Vec<Hit>>, EmbeddingError> {
        let Ok(mut files) = self.files.try_lock() else {
            return Ok(None);
        };

        self.search_in(root, query, limit, &mut files)
            .await
            .map(Some)
    }

    async fn search_in(
        &self,
        root: &Path,
        query: &str,
        limit: usize,
        indexed: &mut Indexed,
    ) -> Result<Vec<Hit>, EmbeddingError> {
        self.refresh(root, indexed).await?;

        let files = &indexed.files;

        let query_vector = self.client.embed_query(query).await?;

        // Sorted by path so that equal scores always come out the same way.
        let mut indexed: Vec<(&PathBuf, &IndexedFile)> = files
            .iter()
            .filter(|(path, _)| path.starts_with(root))
            .collect();

        indexed.sort_by_key(|(path, _)| *path);

        let (chunks, vectors): (Vec<&Chunk>, Vec<&Quantized>) = indexed
            .iter()
            .flat_map(|(_, file)| file.chunks.iter().zip(&file.vectors))
            .unzip();

        let nearest = quant::nearest(
            &vectors,
            &query_vector,
            limit,
            limit * SHORTLIST_PER_RESULT,
            SCAN_ALL_BELOW,
        );

        let hits = nearest
            .into_iter()
            .map(|(position, score)| Hit {
                chunk: chunks[position].clone(),
                score,
            })
            .collect();

        Ok(hits)
    }

    /// Embeds whatever under `root` is new or changed, without searching.
    /// Done ahead of a task, it takes the wait out of the first search.
    pub async fn warm(&self, root: &Path) -> Result<IndexSize, EmbeddingError> {
        let mut files = self.files.lock().await;

        self.refresh(root, &mut files).await?;

        let indexed = files
            .files
            .iter()
            .filter(|(path, _)| path.starts_with(root));

        let mut size = IndexSize::default();

        for (_, file) in indexed {
            size.files += 1;
            size.chunks += file.chunks.len();
            size.bytes += file.vectors.iter().map(Quantized::size).sum::<usize>();
        }

        Ok(size)
    }

    /// Which model the embedding service runs.
    async fn maker(&self) -> Option<&Maker> {
        let asked = self.maker.get_or_init(|| async {
            let health = self.client.health().await.ok()?;

            Some(Maker {
                model: health.model,
                dimension: health.dimension,
            })
        });

        asked.await.as_ref()
    }

    /// Brings the stored vectors in line with the files on disk.
    async fn refresh(&self, root: &Path, indexed: &mut Indexed) -> Result<(), EmbeddingError> {
        let maker = self.maker().await;

        // The first time a project is seen, what an earlier process saved
        // for it is taken over. Files it has already embedded in this one
        // are newer and are kept.
        if let Some(maker) = maker
            && indexed.read_from_disk.insert(root.to_path_buf())
        {
            for (path, file) in store::load(root, maker) {
                indexed.files.entry(path).or_insert(file);
            }
        }

        let files = &mut indexed.files;

        let known_before = files.len();

        let on_disk: Vec<PathBuf> = walk::files_under(root)
            .into_iter()
            .filter(|path| is_searchable(path))
            .collect();

        // Forget files that were deleted.
        let existing: HashSet<&PathBuf> = on_disk.iter().collect();

        files.retain(|path, _| !path.starts_with(root) || existing.contains(path));

        let forgotten = known_before - files.len();

        let mut pending: Vec<(PathBuf, Option<SystemTime>, Vec<Chunk>)> = Vec::new();

        for path in on_disk {
            let modified = modified_time(&path);

            let is_current = files
                .get(&path)
                .is_some_and(|file| file.modified == modified);

            if is_current {
                continue;
            }

            let Ok(source) = fs::read_to_string(&path) else {
                continue;
            };

            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .display()
                .to_string();

            let chunks = chunker::chunk_file(&self.code_index, &path, &relative, &source);

            pending.push((path, modified, chunks));
        }

        // Embed every changed file's chunks together, so indexing a whole
        // project takes a few large requests instead of one per file.
        let all_chunks: Vec<Chunk> = pending
            .iter()
            .flat_map(|(_, _, chunks)| chunks.iter().cloned())
            .collect();

        let changed = forgotten > 0 || !pending.is_empty();

        let mut vectors = self.client.embed_chunks(&all_chunks).await?.into_iter();

        for (path, modified, chunks) in pending {
            let file_vectors: Vec<Quantized> = vectors
                .by_ref()
                .take(chunks.len())
                .map(|vector| Quantized::from_vector(&vector))
                .collect();

            files.insert(
                path,
                IndexedFile {
                    modified,
                    chunks,
                    vectors: file_vectors,
                },
            );
        }

        // Losing the saved copy costs the next process some time and
        // nothing else, so failing to write it is only worth a warning.
        if changed
            && let Some(maker) = maker
            && let Err(error) = store::save(root, maker, files)
        {
            eprintln!(
                "warning: cannot save the search index of {}: {error}",
                root.display()
            );
        }

        Ok(())
    }
}

fn is_searchable(path: &Path) -> bool {
    let is_code = Language::for_path(path).is_some();

    let is_markdown = path.extension().is_some_and(|extension| extension == "md");

    let is_small = fs::metadata(path).is_ok_and(|metadata| metadata.len() <= MAX_FILE_BYTES);

    (is_code || is_markdown) && is_small
}

fn modified_time(path: &Path) -> Option<SystemTime> {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
}
