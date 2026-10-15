//! Reading the engine's memory.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use cortex_brain::memory::store::StoredEpisode;
use cortex_brain::memory::{Embedder, Episode, Fact, Store, StoredLesson};
use cortex_retrieval::EmbeddingClient;

use super::words::hash_words;

/// Longest the app waits for episodes to be embedded. The service may be
/// busy indexing a project for another session.
const EMBED_TIMEOUT: Duration = Duration::from_secs(45);

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
    pub total_episodes: usize,
    /// Oldest first.
    pub episodes: Vec<Episode>,

    /// One vector per episode, all of one length.
    pub vectors: Vec<Vec<f32>>,

    /// Long-term memory.
    pub facts: Vec<Fact>,

    /// What runs here said a later run should know.
    pub lessons: Vec<StoredLesson>,

    pub placement: Placement,
}

/// One project's memory: its file, and the repository it is about.
struct Source {
    store: Store,
    scope: String,
}

/// Row ids are per file. Episodes and lessons of the nth other project
/// are numbered from n times this, so that none is taken for another.
const ID_SPACING: i64 = 1_000_000_000;

pub struct MemoryReader {
    /// The project the agent works in.
    own: Source,

    /// Other projects shown along with it, when memory is combined.
    others: Vec<Source>,

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
            own: Source {
                store,
                scope: scope.to_string(),
            },
            others: Vec::new(),
            embedder,
        })
    }

    /// Shows the memory of the projects in `folders` along with this
    /// one's. Each keeps its own file, in `state_directory` inside it; a
    /// folder that has none yet, or is this project, adds nothing. Only
    /// the picture is combined: the agent still recalls its own project.
    pub fn combined_with(mut self, folders: &[PathBuf], state_directory: &str) -> Self {
        for folder in folders {
            // The engine names a repository by its real path.
            let Ok(folder) = folder.canonicalize() else {
                continue;
            };

            let path = folder.join(state_directory).join("memory.db");
            let scope = folder.display().to_string();

            let known =
                scope == self.own.scope || self.others.iter().any(|other| other.scope == scope);

            if known || !path.is_file() {
                continue;
            }

            if let Ok(store) = Store::open(&path) {
                self.others.push(Source { store, scope });
            }
        }

        self
    }

    pub async fn read(&self) -> Result<Remembered> {
        let mut stored = Vec::new();
        let mut facts: Vec<Fact> = Vec::new();
        let mut lessons: Vec<StoredLesson> = Vec::new();
        let mut total_episodes = 0;

        for (index, source) in [&self.own].into_iter().chain(&self.others).enumerate() {
            let offset = index as i64 * ID_SPACING;

            let mut episodes = self.episodes(source).await?;

            for stored in &mut episodes {
                stored.episode.id += offset;
            }

            stored.extend(episodes);

            for fact in source
                .store
                .facts(&source.scope)
                .context("cannot read the facts")?
            {
                // The same command or note learned in two projects is one fact.
                if !facts
                    .iter()
                    .any(|known| known.kind == fact.kind && known.key == fact.key)
                {
                    facts.push(fact);
                }
            }

            let rows = source
                .store
                .lessons(&source.scope)
                .context("cannot read the lessons")?;

            lessons.extend(rows.into_iter().map(|row| StoredLesson {
                id: row.lesson.id + offset,
                ..row.lesson
            }));

            total_episodes += source.store.episode_count(&source.scope)?;
        }

        // Oldest first, across projects as within one.
        stored.sort_by_key(|stored| stored.episode.created_ms);

        let (episodes, embeddings): (Vec<Episode>, Vec<Option<Vec<f32>>>) = stored
            .into_iter()
            .map(|stored| (stored.episode, stored.embedding))
            .unzip();

        let (vectors, placement) = self.place(&episodes, embeddings);

        Ok(Remembered {
            total_episodes,
            episodes,
            vectors,
            facts,
            lessons,
            placement,
        })
    }

    /// One project's episodes, oldest first, each with its vector when
    /// there is one.
    async fn episodes(&self, source: &Source) -> Result<Vec<StoredEpisode>> {
        let mut stored = source
            .store
            .recent_episodes(&source.scope, 512)
            .context("cannot read the episodes")?;

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
                    let _ = source
                        .store
                        .set_episode_embedding(stored[index].episode.id, &vector);

                    stored[index].embedding = Some(vector);
                }
            }
        }

        Ok(stored)
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

    #[tokio::test]
    async fn combined_memory_shows_other_projects_as_well() {
        let here = tempfile::tempdir().unwrap();
        let there = tempfile::tempdir().unwrap();

        let here_scope = here.path().canonicalize().unwrap().display().to_string();
        let there_scope = there.path().canonicalize().unwrap().display().to_string();

        let file = |folder: &Path| folder.join(".cortex").join("memory.db");

        for (folder, scope) in [(here.path(), &here_scope), (there.path(), &there_scope)] {
            std::fs::create_dir_all(folder.join(".cortex")).unwrap();

            seed::plant(&file(folder), scope).unwrap();
        }

        // Listing this project, or a folder with no memory, adds nothing.
        let folders = vec![
            there.path().to_path_buf(),
            here.path().to_path_buf(),
            std::env::temp_dir().join("cortex-no-such-project"),
        ];

        let alone = MemoryReader::open(&file(here.path()), &here_scope, NO_SERVICE)
            .await
            .unwrap();

        assert_eq!(alone.read().await.unwrap().episodes.len(), seed::count());

        let combined = alone.combined_with(&folders, ".cortex");
        let remembered = combined.read().await.unwrap();

        assert_eq!(remembered.episodes.len(), seed::count() * 2);
        assert_eq!(remembered.total_episodes, seed::count() * 2);
        assert_eq!(remembered.vectors.len(), remembered.episodes.len());

        let mut ids: Vec<i64> = remembered
            .episodes
            .iter()
            .map(|episode| episode.id)
            .collect();

        ids.sort_unstable();
        ids.dedup();

        assert_eq!(ids.len(), remembered.episodes.len());
    }
}
