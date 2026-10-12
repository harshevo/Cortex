//! Reading the engine's memory.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use cortex_brain::intuition::{Network, Picture};
use cortex_cli::features::intuition_file;
use cortex_memory::{Embedder, Episode, Fact, Store, StoredLesson};
use cortex_retrieval::EmbeddingClient;

use super::words::hash_words;

/// Longest the app waits for episodes to be embedded. The service may be
/// busy indexing a project for another session.
const EMBED_TIMEOUT: Duration = Duration::from_secs(45);

/// Most neurons of the intuition drawn. The network may hold thousands;
/// the best connected are the ones worth seeing.
const NEURONS_SHOWN: usize = 60;

/// What decided where each episode sits.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Placement {
    /// The named embedding model: episodes sit by the meaning of the task.
    Model(String),

    /// No model was available: episodes sit by the words they share.
    #[default]
    SharedWords,
}

/// Everything the engine remembers about one repository.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Remembered {
    /// Oldest first.
    pub episodes: Vec<Episode>,

    /// One vector per episode, all of one length.
    pub vectors: Vec<Vec<f32>>,

    /// Long-term memory.
    pub facts: Vec<Fact>,

    /// What runs here said a later run should know.
    pub lessons: Vec<StoredLesson>,

    pub placement: Placement,

    /// The best-connected part of the associative network. Empty when
    /// the intuition has never been switched on here.
    pub intuition: Picture,
}

pub struct MemoryReader {
    store: Store,
    scope: String,

    /// Where the engine keeps the intuition's network.
    intuition: PathBuf,

    /// The embedding service and the model it runs, when it answered.
    embedder: Option<(EmbeddingClient, String)>,
}

impl MemoryReader {
    /// Opens the memory file the agent writes to. `scope` is the
    /// repository the app is working in.
    pub async fn open(path: &Path, scope: &str, embedding_url: &str) -> Result<Self> {
        let store = Store::open(path)
            .with_context(|| format!("cannot open the memory at {}", path.display()))?;

        let client = EmbeddingClient::new(embedding_url);

        let embedder = match client.health().await {
            Ok(health) => Some((client, health.model)),
            Err(_) => None,
        };

        Ok(Self {
            store,
            scope: scope.to_string(),
            intuition: intuition_file(path),
            embedder,
        })
    }

    pub async fn read(&self) -> Result<Remembered> {
        let mut stored = self
            .store
            .episodes(&self.scope)
            .context("cannot read the episodes")?;

        let facts = self
            .store
            .facts(&self.scope)
            .context("cannot read the facts")?;

        let lessons = self
            .store
            .lessons(&self.scope)
            .context("cannot read the lessons")?
            .into_iter()
            .map(|row| row.lesson)
            .collect();

        // The engine embeds an episode the first time it is recalled. The
        // picture wants it placed straight away, so do that part early.
        let missing: Vec<usize> = (0..stored.len())
            .filter(|index| stored[*index].embedding.is_none())
            .collect();

        if let Some((client, _)) = &self.embedder
            && !missing.is_empty()
        {
            let texts: Vec<String> = missing
                .iter()
                .map(|index| stored[*index].episode.search_text())
                .collect();

            let embedded =
                tokio::time::timeout(EMBED_TIMEOUT, Embedder::embed_documents(client, &texts))
                    .await;

            if let Ok(Some(vectors)) = embedded {
                for (index, vector) in missing.into_iter().zip(vectors) {
                    // A vector that cannot be saved is only worked out again.
                    let _ = self
                        .store
                        .set_episode_embedding(stored[index].episode.id, &vector);

                    stored[index].embedding = Some(vector);
                }
            }
        }

        let (episodes, embeddings): (Vec<Episode>, Vec<Option<Vec<f32>>>) = stored
            .into_iter()
            .map(|stored| (stored.episode, stored.embedding))
            .unzip();

        let (vectors, placement) = self.place(&episodes, embeddings);

        Ok(Remembered {
            episodes,
            vectors,
            facts,
            lessons,
            placement,
            intuition: Network::load_or_new(&self.intuition).picture(NEURONS_SHOWN),
        })
    }

    /// Vectors from different sources cannot be compared, so either every
    /// episode is placed by the model or every one by its words.
    fn place(
        &self,
        episodes: &[Episode],
        embeddings: Vec<Option<Vec<f32>>>,
    ) -> (Vec<Vec<f32>>, Placement) {
        let by_model: Option<Vec<Vec<f32>>> = embeddings.into_iter().collect();

        let comparable = |vectors: &Vec<Vec<f32>>| {
            vectors
                .first()
                .is_none_or(|first| vectors.iter().all(|vector| vector.len() == first.len()))
        };

        match (by_model, &self.embedder) {
            (Some(vectors), Some((_, model))) if comparable(&vectors) => {
                (vectors, Placement::Model(model.clone()))
            }

            _ => {
                let vectors = episodes
                    .iter()
                    .map(|episode| hash_words(&episode.search_text()))
                    .collect();

                (vectors, Placement::SharedWords)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::seed;

    /// An address nothing listens on, so the reader has no embedder.
    const NO_SERVICE: &str = "http://127.0.0.1:9";

    #[tokio::test]
    async fn what_the_engine_stored_is_read_back() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("memory.db");

        seed::plant(&path, "demo").unwrap();

        let reader = MemoryReader::open(&path, "demo", NO_SERVICE).await.unwrap();
        let remembered = reader.read().await.unwrap();

        assert_eq!(remembered.episodes.len(), seed::count());
        assert_eq!(remembered.vectors.len(), seed::count());
        assert_eq!(remembered.placement, Placement::SharedWords);

        // Files that several seeded tasks changed were promoted to facts.
        assert!(!remembered.facts.is_empty());
    }

    #[tokio::test]
    async fn another_repository_has_its_own_memory() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("memory.db");

        seed::plant(&path, "demo").unwrap();

        let reader = MemoryReader::open(&path, "elsewhere", NO_SERVICE)
            .await
            .unwrap();

        assert!(reader.read().await.unwrap().episodes.is_empty());
    }
}
