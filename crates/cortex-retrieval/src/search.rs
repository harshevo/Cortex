//! The vectors of a project, and ranking them against a query.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use cortex_index::{CodeIndex, Language, walk};
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use tokio::sync::{Mutex, OnceCell, OwnedMutexGuard, Semaphore};

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
    files: Arc<Mutex<Indexed>>,

    /// The model behind the embedding service, asked once. `None` when it
    /// could not be asked: nothing is read from or saved to disk then.
    maker: OnceCell<Option<Maker>>,
}

impl SemanticIndex {
    pub fn new(client: EmbeddingClient, code_index: Arc<CodeIndex>) -> Self {
        Self {
            client,
            code_index,
            files: Arc::new(Mutex::new(Indexed::default())),
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
        let files = self.files.clone().lock_owned().await;

        self.search_in(root, query, limit, files).await
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
        let Ok(files) = self.files.clone().try_lock_owned() else {
            return Ok(None);
        };

        self.search_in(root, query, limit, files).await.map(Some)
    }

    async fn search_in(
        &self,
        root: &Path,
        query: &str,
        limit: usize,
        indexed: OwnedMutexGuard<Indexed>,
    ) -> Result<Vec<Hit>, EmbeddingError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let indexed = self.refresh(root, indexed).await?;
        let query_vector = self.client.embed_query(query).await?;
        let root = root.to_path_buf();
        blocking(move || {
            let mut files: Vec<_> = indexed
                .files
                .iter()
                .filter(|(path, _)| path.starts_with(&root))
                .collect();
            files.sort_by_key(|(path, _)| *path);
            let (chunks, vectors): (Vec<_>, Vec<_>) = files
                .iter()
                .flat_map(|(_, file)| file.chunks.iter().zip(&file.vectors))
                .unzip();
            quant::nearest(
                &vectors,
                &query_vector,
                limit,
                limit.saturating_mul(SHORTLIST_PER_RESULT),
                SCAN_ALL_BELOW,
            )
            .into_iter()
            .map(|(position, score)| Hit {
                chunk: chunks[position].clone(),
                score,
            })
            .collect()
        })
        .await
    }

    pub async fn warm(&self, root: &Path) -> Result<IndexSize, EmbeddingError> {
        let files = self.files.clone().lock_owned().await;
        let indexed = self.refresh(root, files).await?;
        let root = root.to_path_buf();
        blocking(move || {
            let mut size = IndexSize::default();
            for (_, file) in indexed
                .files
                .iter()
                .filter(|(path, _)| path.starts_with(&root))
            {
                size.files += 1;
                size.chunks += file.chunks.len();
                size.bytes += file.vectors.iter().map(Quantized::size).sum::<usize>();
            }
            size
        })
        .await
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
    async fn refresh(
        &self,
        root: &Path,
        indexed: OwnedMutexGuard<Indexed>,
    ) -> Result<OwnedMutexGuard<Indexed>, EmbeddingError> {
        let maker = self.maker().await.cloned();
        let root = root.to_path_buf();
        let code_index = self.code_index.clone();
        let (mut indexed, root, maker, pending, all_chunks, changed) = blocking(move || {
            let mut indexed = indexed;
            if let Some(maker) = &maker
                && indexed.read_from_disk.insert(root.clone())
            {
                for (path, file) in store::load(&root, maker) {
                    indexed.files.entry(path).or_insert(file);
                }
            }
            let files = &mut indexed.files;
            let known_before = files.len();
            let on_disk: Vec<PathBuf> = walk::files_under(&root)
                .into_iter()
                .filter(|path| is_searchable(path))
                .collect();
            let existing: HashSet<_> = on_disk.iter().collect();
            files.retain(|path, _| !path.starts_with(&root) || existing.contains(path));
            let forgotten = known_before - files.len();
            let mut pending = Vec::new();
            for path in on_disk {
                let modified = modified_time(&path);
                if files
                    .get(&path)
                    .is_some_and(|file| file.modified == modified)
                {
                    continue;
                }
                let Ok(source) = fs::read_to_string(&path) else {
                    continue;
                };
                let relative = path
                    .strip_prefix(&root)
                    .unwrap_or(&path)
                    .display()
                    .to_string();
                let chunks = chunker::chunk_file(&code_index, &path, &relative, &source);
                pending.push((path, modified, chunks));
            }
            let changed = forgotten > 0 || !pending.is_empty();
            let all_chunks: Vec<_> = pending
                .iter()
                .flat_map(|(_, _, chunks)| chunks.iter().cloned())
                .collect();
            (indexed, root, maker, pending, all_chunks, changed)
        })
        .await?;
        let vectors = self.client.embed_chunks(&all_chunks).await?;
        drop(all_chunks);
        blocking(move || {
            let mut vectors = vectors.into_iter();
            for (path, modified, chunks) in pending {
                let vectors = vectors
                    .by_ref()
                    .take(chunks.len())
                    .map(|vector| Quantized::from_vector(&vector))
                    .collect();
                indexed.files.insert(
                    path,
                    IndexedFile {
                        modified,
                        chunks,
                        vectors,
                    },
                );
            }
            if changed
                && let Some(maker) = maker
                && let Err(error) = store::save(&root, &maker, &indexed.files)
            {
                eprintln!(
                    "warning: cannot save the search index of {}: {error}",
                    root.display()
                );
            }
            indexed
        })
        .await
    }
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, EmbeddingError> {
    static LIMIT: OnceLock<Arc<Semaphore>> = OnceLock::new();
    let permit = LIMIT
        .get_or_init(|| Arc::new(Semaphore::new(2)))
        .clone()
        .acquire_owned()
        .await
        .map_err(|error| EmbeddingError::Service(error.to_string()))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work()
    })
    .await
    .map_err(|error| EmbeddingError::Service(format!("index worker failed: {error}")))
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

#[cfg(test)]
mod worker_tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct Server {
        url: String,
        stop: Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }
    impl Server {
        fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            listener.set_nonblocking(true).unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let shutdown = stop.clone();
            let thread = std::thread::spawn(move || {
                while !shutdown.load(Ordering::Relaxed) {
                    let Ok((mut stream, _)) = listener.accept() else {
                        std::thread::sleep(std::time::Duration::from_millis(2));
                        continue;
                    };
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                        .unwrap();
                    let mut reader = BufReader::new(&mut stream);
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    let health = line.starts_with("GET /health");
                    let mut length = 0;
                    loop {
                        line.clear();
                        reader.read_line(&mut line).unwrap();
                        if line == "\r\n" || line.is_empty() {
                            break;
                        }
                        if let Some((key, value)) = line.split_once(':')
                            && key.eq_ignore_ascii_case("content-length")
                        {
                            length = value.trim().parse::<usize>().unwrap();
                        }
                    }
                    let mut body = vec![0; length];
                    reader.read_exact(&mut body).unwrap();
                    let response = if health {
                        serde_json::json!({"model":"test", "dimension":3, "device":"cpu"})
                    } else {
                        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
                        let count = body
                            .get("documents")
                            .or_else(|| body.get("texts"))
                            .unwrap()
                            .as_array()
                            .unwrap()
                            .len();
                        serde_json::json!({"vectors": vec![vec![1.0, 0.0, 0.0]; count]})
                    }
                    .to_string();
                    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", response.len(), response).unwrap();
                }
            });
            Self {
                url,
                stop,
                thread: Some(thread),
            }
        }
    }
    impl Drop for Server {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            let _ = self.thread.take().unwrap().join();
        }
    }

    #[tokio::test]
    async fn indexing_search_and_persistence_survive_offloading() {
        let server = Server::start();
        let project = tempfile::tempdir().unwrap();
        let source = project.path().join("entry.py");
        fs::write(&source, "def original():\n    return 1\n").unwrap();
        let index = crate::SemanticIndex::new(
            EmbeddingClient::new(&server.url),
            Arc::new(CodeIndex::new()),
        );
        let guard = index.files.clone().lock_owned().await;
        assert!(
            index
                .search_unless_busy(project.path(), "original", 3)
                .await
                .unwrap()
                .is_none()
        );
        drop(guard);
        let warm = index.warm(project.path()).await.unwrap();
        assert_eq!(warm.files, 1);
        assert!(warm.chunks > 0);
        let hits = index.search(project.path(), "original", 3).await.unwrap();
        assert!(!hits.is_empty());
        assert!(hits[0].chunk.text.contains("original"));
        let saved = project.path().join(".cortex/semantic-index.json");
        assert!(saved.exists());
        let loaded = crate::SemanticIndex::new(
            EmbeddingClient::new(&server.url),
            Arc::new(CodeIndex::new()),
        );
        assert_eq!(loaded.warm(project.path()).await.unwrap(), warm);
        fs::write(&source, "def updated():\n    return 2\n").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&source)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5)),
            )
            .unwrap();
        assert!(
            index.search(project.path(), "updated", 3).await.unwrap()[0]
                .chunk
                .text
                .contains("updated")
        );
        fs::remove_file(source).unwrap();
        assert!(
            index
                .search(project.path(), "original", 3)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(index.warm(project.path()).await.unwrap().files, 0);
    }

    #[tokio::test]
    async fn a_blocking_index_job_leaves_the_async_executor_available() {
        let (started, ready) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let job = tokio::spawn(blocking(move || {
            started.send(()).unwrap();
            wait.recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
        }));
        ready.await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        assert!(!job.is_finished());
        release.send(()).unwrap();
        job.await.unwrap().unwrap();
    }
}
