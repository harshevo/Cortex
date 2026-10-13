//! The memory system: the three tiers behind the interfaces the agent
//! loop already has.
//!
//! - As a [`TrajectorySink`] it watches a run, which fills the working
//!   memory, and consolidates it when the run finishes.
//! - As a [`ContextSource`] it recalls earlier work before a run starts.
//!
//! It never changes a file and never stops a run: when the store or the
//! embedding service fails, the agent simply works without memory.

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use cortex_core::trajectory::unix_millis;
use cortex_core::{Briefing, ContextSource, Event, TrajectorySink, Workspace};

use crate::episode::Episode;
use crate::fact::{Fact, FactKind};
use crate::lesson::StoredLesson;
use crate::recall::{self, Recall, ScoredEpisode, ScoredLesson, TrackRecord};
use crate::similarity::{self, Embedder, Reranker};
use crate::store::{LessonRow, MemoryError, Store, StoredEpisode};
use crate::working::WorkingMemory;

#[derive(Debug, Clone)]
pub struct MemoryConfig {
    /// Separates repositories that share one store.
    pub scope: String,

    /// Episodes shown for one task at most.
    pub max_episodes: usize,

    /// Leave out episodes whose instruction is the same as the task's.
    /// Evaluation runs repeat tasks; recalling the previous attempt at the
    /// same task would measure copying, not memory of the repository.
    pub skip_identical: bool,

    /// Show how often earlier tasks here passed their checks first time.
    pub show_track_record: bool,
}

impl MemoryConfig {
    pub fn for_scope(scope: impl Into<String>) -> Self {
        Self {
            scope: scope.into(),
            max_episodes: 3,
            skip_identical: false,
            show_track_record: false,
        }
    }
}

/// Lowest similarity at which an episode is worth showing. The two
/// measures have different ranges, so each has its own floor.
const EMBEDDING_FLOOR: f32 = 0.25;
const LEXICAL_FLOOR: f32 = 0.08;

/// Another project's memory. It is read, and never added to: only the
/// vectors of its episodes are filled in, as that project would itself.
struct Other {
    store: Store,
    scope: String,

    /// The project's folder name, to say where a memory is from.
    name: String,
}

pub struct MemorySystem {
    store: Store,
    config: MemoryConfig,

    /// Other projects whose memory is recalled along with this one's.
    others: Vec<Other>,
    embedder: Option<Arc<dyn Embedder>>,
    reranker: Option<Arc<dyn Reranker>>,
    working: Mutex<WorkingMemory>,

    /// What was recalled at the start of the current run, kept so that
    /// the run's outcome can show which of it was useful.
    shown: Mutex<Option<Shown>>,
}

/// The first recall of a run, as vectors.
struct Shown {
    query: Vec<f32>,

    /// For each episode shown: the files it touched, and its vector.
    episodes: Vec<(Vec<String>, Vec<f32>)>,
}

impl MemorySystem {
    pub fn new(store: Store, config: MemoryConfig) -> Self {
        Self {
            store,
            config,
            others: Vec::new(),
            embedder: None,
            reranker: None,
            working: Mutex::new(WorkingMemory::default()),
            shown: Mutex::new(None),
        }
    }

    pub fn open(path: &Path, config: MemoryConfig) -> Result<Self, MemoryError> {
        Ok(Self::new(Store::open(path)?, config))
    }

    /// Compare tasks with an embedding model instead of by shared words.
    pub fn with_embedder(mut self, embedder: Arc<dyn Embedder>) -> Self {
        self.embedder = Some(embedder);

        self
    }

    /// Let a learned model reorder the shortlisted episodes.
    pub fn with_reranker(mut self, reranker: Arc<dyn Reranker>) -> Self {
        self.reranker = Some(reranker);

        self
    }

    /// Also recall what was learned in another project: the tasks there
    /// most like the one at hand, and advice that is not about any one
    /// file. Its facts stay its own, since they are about its code. The
    /// project this memory is for adds nothing.
    pub fn with_other(mut self, store: Store, scope: &str, name: &str) -> Self {
        let known =
            scope == self.config.scope || self.others.iter().any(|other| other.scope == scope);

        if !known {
            self.others.push(Other {
                store,
                scope: scope.to_string(),
                name: name.to_string(),
            });
        }

        self
    }

    /// How many other projects are recalled from.
    pub fn others(&self) -> usize {
        self.others.len()
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn scope(&self) -> &str {
        &self.config.scope
    }

    fn working(&self) -> MutexGuard<'_, WorkingMemory> {
        self.working
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// What the current run has done so far.
    pub fn working_digest(&self) -> String {
        self.working().digest()
    }

    /// Stores a fact the agent wants kept for later tasks.
    pub fn remember(&self, text: &str, paths: Vec<String>) -> Result<(), MemoryError> {
        self.working().note(text);

        let fact = Fact::note(text, paths, unix_millis() as u64);

        self.store.put_fact(&self.config.scope, &fact)
    }

    /// Stores the description of the project that every task is shown,
    /// in place of any earlier one.
    pub fn set_overview(&self, text: &str) -> Result<(), MemoryError> {
        let fact = Fact::overview(text, unix_millis() as u64);

        self.store.put_fact(&self.config.scope, &fact)
    }

    /// The memories relevant to `query`, checked against the files in
    /// `root`.
    pub async fn recall(&self, query: &str, root: &Path) -> Result<Recall, MemoryError> {
        let exists = |path: &str| root.join(path).exists();

        let mut stored = self.store.episodes(&self.config.scope)?;

        if self.config.skip_identical {
            stored.retain(|stored| stored.episode.instruction.trim() != query.trim());
        }

        let query_vector = self.embed_missing(query, &mut stored).await;

        let track_record = if self.config.show_track_record {
            let episodes: Vec<Episode> =
                stored.iter().map(|stored| stored.episode.clone()).collect();

            TrackRecord::from_episodes(&episodes)
        } else {
            None
        };

        let lessons = self
            .recall_lessons(query, query_vector.as_deref(), exists)
            .await?;

        let method = if query_vector.is_some() {
            "embedding"
        } else {
            "lexical"
        };

        let floor = if query_vector.is_some() {
            EMBEDDING_FLOOR
        } else {
            LEXICAL_FLOOR
        };

        let mut candidates = Vec::new();
        let mut vector_by_id = std::collections::HashMap::new();

        for stored in stored {
            let score = match (&query_vector, &stored.embedding) {
                (Some(query_vector), Some(vector)) => similarity::cosine(query_vector, vector),
                _ => similarity::lexical(query, &stored.episode.search_text()),
            };

            if let Some(vector) = &stored.embedding {
                vector_by_id.insert(stored.episode.id, vector.clone());
            }

            candidates.push(ScoredEpisode {
                episode: stored.episode,
                score,
            });
        }

        // Which memories are near enough to show is decided by similarity,
        // which is what the floors were set on. The reranker's scores are
        // on a scale of their own, so it only decides the order, and with
        // it which of them fit.
        let mut episodes = recall::select_episodes(candidates, usize::MAX, floor);

        let vectors: Vec<Vec<f32>> = episodes
            .iter()
            .map(|candidate| {
                vector_by_id
                    .get(&candidate.episode.id)
                    .cloned()
                    .unwrap_or_default()
            })
            .collect();

        let reranked = self
            .rerank(query_vector.as_deref(), &vectors, &mut episodes)
            .await;

        if reranked {
            episodes.sort_by(|left, right| right.score.total_cmp(&left.score));
        }

        episodes.truncate(self.config.max_episodes);

        if let Some(query_vector) = &query_vector {
            self.keep_shown(query_vector, &episodes, &vector_by_id);
        }

        let (episodes_elsewhere, lessons_elsewhere) =
            self.recall_elsewhere(query, query_vector.as_deref()).await;

        let recalled = Recall {
            episodes,
            facts: self.current_facts(exists)?,
            lessons,
            episodes_elsewhere,
            lessons_elsewhere,
            track_record,
            method,
            reranked,
        };

        Ok(recalled)
    }

    /// What the other projects hold that is close to `query`. A project
    /// whose memory cannot be read is passed over: it is extra, and must
    /// not cost the task its own memory.
    async fn recall_elsewhere(
        &self,
        query: &str,
        query_vector: Option<&[f32]>,
    ) -> (Vec<(String, ScoredEpisode)>, Vec<(String, ScoredLesson)>) {
        let floor = if query_vector.is_some() {
            EMBEDDING_FLOOR
        } else {
            LEXICAL_FLOOR
        };

        let mut episodes = Vec::new();
        let mut lessons = Vec::new();

        for other in &self.others {
            let mut stored = other.store.episodes(&other.scope).unwrap_or_default();

            if self.config.skip_identical {
                stored.retain(|stored| stored.episode.instruction.trim() != query.trim());
            }

            if query_vector.is_some() {
                self.embed_episodes(&other.store, &mut stored).await;
            }

            for stored in stored {
                let score = match (query_vector, &stored.embedding) {
                    (Some(query_vector), Some(vector)) => similarity::cosine(query_vector, vector),
                    (Some(_), None) => continue,
                    (None, _) => similarity::lexical(query, &stored.episode.search_text()),
                };

                episodes.push((
                    other.name.clone(),
                    ScoredEpisode {
                        episode: stored.episode,
                        score,
                    },
                ));
            }

            // Advice about a file is advice about that project's file.
            let rows: Vec<LessonRow> = other
                .store
                .lessons(&other.scope)
                .unwrap_or_default()
                .into_iter()
                .filter(|row| row.lesson.lesson.paths.is_empty())
                .collect();

            for row in rows {
                let score = match (query_vector, &row.embedding) {
                    (Some(query_vector), Some(vector)) => similarity::cosine(query_vector, vector),
                    (Some(_), None) => continue,
                    (None, _) => similarity::lexical(query, &row.lesson.search_text()),
                };

                lessons.push((
                    other.name.clone(),
                    ScoredLesson {
                        lesson: row.lesson,
                        score,
                    },
                ));
            }
        }

        (
            recall::select_elsewhere(episodes, |scored| scored.score, floor),
            recall::select_elsewhere(lessons, |scored| scored.score, floor),
        )
    }

    /// Remembers the run's first recall. Later recalls in the same run
    /// (the agent using the `recall` tool) do not replace it.
    fn keep_shown(
        &self,
        query_vector: &[f32],
        episodes: &[ScoredEpisode],
        vector_by_id: &std::collections::HashMap<i64, Vec<f32>>,
    ) {
        let mut shown = self
            .shown
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if shown.is_some() {
            return;
        }

        let episodes = episodes
            .iter()
            .filter_map(|scored| {
                let episode = &scored.episode;

                let mut files = episode.files_changed.clone();

                files.extend(episode.files_read.iter().cloned());

                Some((files, vector_by_id.get(&episode.id)?.clone()))
            })
            .collect();

        *shown = Some(Shown {
            query: query_vector.to_vec(),
            episodes,
        });
    }

    /// Tells the learned reranker which of the memories shown at the start
    /// of the run proved useful: those about a file the run went on to
    /// change. A run whose checks failed teaches nothing here.
    fn report_useful_memories(&self) {
        let shown = self
            .shown
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();

        let (Some(shown), Some(reranker)) = (shown, self.reranker.clone()) else {
            return;
        };

        let (changed, failed) = {
            let working = self.working();

            (
                working.files_changed.clone(),
                working.verified == Some(false),
            )
        };

        if failed {
            return;
        }

        let useful: Vec<Vec<f32>> = shown
            .episodes
            .into_iter()
            .filter(|(files, _)| files.iter().any(|file| changed.contains(file)))
            .map(|(_, vector)| vector)
            .collect();

        if useful.is_empty() {
            return;
        }

        let report = async move {
            for vector in &useful {
                reranker.observe(&shown.query, vector).await;
            }
        };

        // Events arrive on the agent's own async runtime. Wait for the
        // report where that is possible, because a `run` command exits as
        // soon as the run is over.
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };

        if runtime.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread {
            tokio::task::block_in_place(|| runtime.block_on(report));
        } else {
            runtime.spawn(report);
        }
    }

    /// The lessons closest to `query`. A lesson about a file that is gone
    /// is deleted on the way.
    async fn recall_lessons(
        &self,
        query: &str,
        query_vector: Option<&[f32]>,
        exists: impl Fn(&str) -> bool + Copy,
    ) -> Result<Vec<ScoredLesson>, MemoryError> {
        let mut rows = Vec::new();

        for row in self.store.lessons(&self.config.scope)? {
            if row.lesson.still_holds(exists) {
                rows.push(row);
            } else {
                self.store.delete_lesson(row.lesson.id)?;
            }
        }

        if query_vector.is_some() {
            self.embed_lessons(&mut rows).await;
        }

        let floor = if query_vector.is_some() {
            EMBEDDING_FLOOR
        } else {
            LEXICAL_FLOOR
        };

        let scored = rows
            .into_iter()
            .map(|row| {
                let score = match (query_vector, &row.embedding) {
                    (Some(query_vector), Some(vector)) => similarity::cosine(query_vector, vector),
                    _ => similarity::lexical(query, &row.lesson.search_text()),
                };

                ScoredLesson {
                    lesson: row.lesson,
                    score,
                }
            })
            .collect();

        Ok(recall::select_lessons(scored, floor))
    }

    /// Embeds the lessons that have no vector yet.
    async fn embed_lessons(&self, rows: &mut [LessonRow]) {
        let Some(embedder) = &self.embedder else {
            return;
        };

        let missing: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.embedding.is_none())
            .map(|(position, _)| position)
            .collect();

        if missing.is_empty() {
            return;
        }

        let texts: Vec<String> = missing
            .iter()
            .map(|&position| rows[position].lesson.search_text())
            .collect();

        let Some(vectors) = embedder.embed_documents(&texts).await else {
            return;
        };

        for (&position, vector) in missing.iter().zip(vectors) {
            let _ = self
                .store
                .set_lesson_embedding(rows[position].lesson.id, &vector);

            rows[position].embedding = Some(vector);
        }
    }

    /// Keeps what a run taught. Advice already held is counted again
    /// instead of being stored twice.
    fn learn(&self, lessons: &[cortex_core::Lesson]) -> Result<(), MemoryError> {
        let scope = &self.config.scope;

        let mut held: Vec<StoredLesson> = self
            .store
            .lessons(scope)?
            .into_iter()
            .map(|row| row.lesson)
            .collect();

        for lesson in lessons {
            match held.iter_mut().find(|held| held.same_advice(lesson)) {
                Some(same) => {
                    same.support += 1;

                    self.store.update_lesson(same)?;
                }

                None => {
                    let mut stored = StoredLesson::new(lesson.clone(), unix_millis() as u64);

                    stored.id = self.store.add_lesson(scope, &stored)?;

                    held.push(stored);
                }
            }
        }

        Ok(())
    }

    /// Embeds the query and any episode that has no vector yet. Returns
    /// the query's vector, or `None` when embeddings cannot be used.
    async fn embed_missing(&self, query: &str, stored: &mut [StoredEpisode]) -> Option<Vec<f32>> {
        let embedder = self.embedder.as_ref()?;

        self.embed_episodes(&self.store, stored).await?;

        embedder.embed_query(query).await
    }

    /// Gives every episode of `store` that has no vector yet one, and
    /// saves it there. `None` when embeddings cannot be used.
    async fn embed_episodes(&self, store: &Store, stored: &mut [StoredEpisode]) -> Option<()> {
        let embedder = self.embedder.as_ref()?;

        let missing: Vec<usize> = stored
            .iter()
            .enumerate()
            .filter(|(_, stored)| stored.embedding.is_none())
            .map(|(position, _)| position)
            .collect();

        if !missing.is_empty() {
            let texts: Vec<String> = missing
                .iter()
                .map(|&position| stored[position].episode.search_text())
                .collect();

            let vectors = embedder.embed_documents(&texts).await?;

            for (&position, vector) in missing.iter().zip(vectors) {
                // A vector that cannot be saved is only recomputed later.
                let _ = store.set_episode_embedding(stored[position].episode.id, &vector);

                stored[position].embedding = Some(vector);
            }
        }

        Some(())
    }

    /// Lets the learned reranker replace the similarity scores. Returns
    /// whether it did.
    async fn rerank(
        &self,
        query_vector: Option<&[f32]>,
        vectors: &[Vec<f32>],
        candidates: &mut [ScoredEpisode],
    ) -> bool {
        let (Some(reranker), Some(query_vector)) = (&self.reranker, query_vector) else {
            return false;
        };

        if candidates.is_empty() || vectors.iter().any(Vec::is_empty) {
            return false;
        }

        let Some(scores) = reranker.rerank(query_vector, vectors).await else {
            return false;
        };

        if scores.len() != candidates.len() {
            return false;
        }

        for (candidate, score) in candidates.iter_mut().zip(scores) {
            candidate.score = score;
        }

        true
    }

    /// The facts that still hold. A note about a file that no longer
    /// exists is deleted; derived facts are rebuilt after every task, so
    /// they are only skipped.
    fn current_facts(
        &self,
        exists: impl Fn(&str) -> bool + Copy,
    ) -> Result<Vec<Fact>, MemoryError> {
        let mut current = Vec::new();

        for fact in self.store.facts(&self.config.scope)? {
            if fact.still_holds(exists) {
                current.push(fact);
            } else if fact.kind == FactKind::Note {
                self.store.delete_fact(&self.config.scope, &fact)?;
            }
        }

        Ok(recall::select_facts(current))
    }

    /// Consolidation: turns the finished run into an episode and rebuilds
    /// the facts derived from all episodes.
    fn consolidate(&self) -> Result<(), MemoryError> {
        let now = unix_millis() as u64;

        let episode = Episode::from_working(&self.working(), now);

        if episode.is_empty() {
            return Ok(());
        }

        let scope = &self.config.scope;

        self.store.consolidate_episode(scope, &episode, now)
    }
}

impl TrajectorySink for MemorySystem {
    fn record(&self, event: &Event) {
        self.working().observe(event);

        match event {
            Event::RunStarted { .. } => {
                *self
                    .shown
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
            }

            Event::LessonsLearned { lessons, .. } => {
                if let Err(error) = self.learn(lessons) {
                    eprintln!("warning: the lessons of this task were not saved: {error}");
                }
            }

            Event::RunFinished { .. } => {
                self.report_useful_memories();

                if let Err(error) = self.consolidate() {
                    eprintln!("warning: this task was not saved to memory: {error}");
                }
            }

            _ => {}
        }
    }
}

#[async_trait]
impl ContextSource for MemorySystem {
    async fn brief(&self, instruction: &str, workspace: &Workspace) -> Option<Briefing> {
        let root = workspace.root();

        let recalled = match self.recall(instruction, root).await {
            Ok(recalled) => recalled,

            Err(error) => {
                eprintln!("warning: memory could not be read: {error}");

                return None;
            }
        };

        if recalled.is_empty() {
            return None;
        }

        let detail = recalled.detail();

        let _ = self.store.log_recall(
            &self.config.scope,
            unix_millis() as u64,
            instruction,
            &detail,
        );

        Some(Briefing {
            source: "memory".to_string(),
            text: recalled.render(|path| root.join(path).exists()),
            detail,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use cortex_core::{CheckOutcome, Outcome, ToolCall, ToolResult, Usage};
    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn chat_only_answers_and_calculations_survive_restart_and_recall() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("memory.db");
        let memory = MemorySystem::open(&file, MemoryConfig::for_scope("chat")).unwrap();
        memory.record(&Event::RunStarted {
            instruction: "calculate calendar capacity".into(),
            provider: "scripted".into(),
            model: "scripted".into(),
        });
        let call = ToolCall {
            id: "math".into(),
            name: "calculate".into(),
            input: json!({"expression":"7*24"}),
        };
        memory.record(&Event::ToolStarted { step: 1, call });
        memory.record(&Event::ToolFinished {
            step: 1,
            tool: "calculate".into(),
            result: ToolResult {
                call_id: "math".into(),
                output: "7*24 = 168".into(),
                is_error: false,
                images: Vec::new(),
            },
            duration_ms: 0,
        });
        memory.record(&Event::ModelResponded {
            step: 2,
            message: cortex_core::Message::assistant_text("Calendar capacity is 168 hours."),
            stop_reason: cortex_core::StopReason::EndTurn,
            usage: Usage::default(),
            duration_ms: 0,
        });
        memory.record(&Event::RunFinished {
            outcome: Outcome::Completed,
            steps: 2,
            usage: Usage::default(),
            duration_ms: 1,
            verified: None,
        });
        drop(memory);
        let memory = MemorySystem::open(&file, MemoryConfig::for_scope("chat")).unwrap();
        let rows = memory.store().episodes("chat").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].episode.calculations, ["7*24 = 168"]);
        let recalled = memory
            .recall("calendar capacity", directory.path())
            .await
            .unwrap();
        assert!(recalled.render(|_| true).contains("7*24 = 168"));
        // A following turn without any tools is an episode as well.
        memory.record(&Event::RunStarted {
            instruction: "explain calendar capacity".into(),
            provider: "scripted".into(),
            model: "scripted".into(),
        });
        memory.record(&Event::ModelResponded {
            step: 1,
            message: cortex_core::Message::assistant_text("A calendar week has seven days."),
            stop_reason: cortex_core::StopReason::EndTurn,
            usage: Usage::default(),
            duration_ms: 0,
        });
        memory.record(&Event::RunFinished {
            outcome: Outcome::Completed,
            steps: 1,
            usage: Usage::default(),
            duration_ms: 1,
            verified: None,
        });
        assert_eq!(memory.store().episodes("chat").unwrap().len(), 2);
    }

    #[test]
    fn concurrent_consolidation_keeps_all_episode_support() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("memory.db");
        let memories: Vec<_> = (0..8)
            .map(|_| MemorySystem::open(&path, MemoryConfig::for_scope("shared")).unwrap())
            .collect();
        std::thread::scope(|scope| {
            for memory in memories {
                scope
                    .spawn(move || play_run(&memory, "repair calendar", "calendar.ts", Some(true)));
            }
        });
        let memory = MemorySystem::open(&path, MemoryConfig::for_scope("shared")).unwrap();
        assert_eq!(memory.store().episodes("shared").unwrap().len(), 8);
        let facts = memory.store().facts("shared").unwrap();
        assert_eq!(
            facts
                .iter()
                .find(|fact| fact.kind == FactKind::HotFile)
                .unwrap()
                .support,
            8
        );
    }

    /// Plays a whole run into the memory system as the agent loop would.
    fn play_run(memory: &MemorySystem, instruction: &str, changed: &str, verified: Option<bool>) {
        memory.record(&Event::RunStarted {
            instruction: instruction.to_string(),
            provider: "scripted".to_string(),
            model: "scripted".to_string(),
        });

        play_tools_and_finish(memory, changed, verified);
    }

    /// The part of a run after it has started.
    fn play_tools_and_finish(memory: &MemorySystem, changed: &str, verified: Option<bool>) {
        let tools = [
            ("read_file", json!({ "path": changed }), "..."),
            ("edit_file", json!({ "path": changed }), "edited"),
            ("shell", json!({ "command": "npm test" }), "exit code: 0"),
        ];

        for (position, (tool, input, output)) in tools.into_iter().enumerate() {
            let id = position.to_string();

            memory.record(&Event::ToolStarted {
                step: 1,
                call: ToolCall {
                    id: id.clone(),
                    name: tool.to_string(),
                    input,
                },
            });

            memory.record(&Event::ToolFinished {
                step: 1,
                tool: tool.to_string(),
                result: ToolResult {
                    call_id: id,
                    output: output.to_string(),
                    is_error: false,
                    images: Vec::new(),
                },
                duration_ms: 1,
            });
        }

        if let Some(passed) = verified {
            memory.record(&Event::VerificationRan {
                step: 2,
                passed,
                checks: vec![CheckOutcome {
                    command: "CI=1 npm run test".to_string(),
                    passed,
                    duration_ms: 1,
                    output: String::new(),
                }],
                duration_ms: 1,
            });
        }

        memory.record(&Event::RunFinished {
            outcome: Outcome::Completed,
            steps: 4,
            usage: Usage::default(),
            duration_ms: 1,
            verified,
        });
    }

    fn system() -> MemorySystem {
        MemorySystem::new(Store::in_memory().unwrap(), MemoryConfig::for_scope("repo"))
    }

    fn workspace_with(files: &[&str]) -> (tempfile::TempDir, Workspace) {
        let dir = tempfile::tempdir().unwrap();

        for file in files {
            fs::write(dir.path().join(file), "").unwrap();
        }

        let workspace = Workspace::new(dir.path()).unwrap();

        (dir, workspace)
    }

    #[tokio::test]
    async fn lessons_are_kept_once_recalled_for_a_like_task_and_dropped_when_stale() {
        let memory = system();
        let (dir, workspace) = workspace_with(&["money.ts"]);

        let lesson = |advice: &str| cortex_core::Lesson {
            title: "Money formatting".to_string(),
            when: "a money amount is displayed wrongly".to_string(),
            advice: advice.to_string(),
            paths: vec!["money.ts".to_string()],
            from_failure: true,
        };

        let learned = |advice: &str| Event::LessonsLearned {
            lessons: vec![lesson(advice)],
            usage: Usage::default(),
            duration_ms: 1,
        };

        memory.record(&learned(
            "Formatting of amounts lives in money.ts; look there first.",
        ));
        memory.record(&learned(
            "Look in money.ts first; formatting of amounts lives there.",
        ));

        let stored = memory.store().lessons("repo").unwrap();

        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].lesson.support, 2);

        let briefing = memory
            .brief("A negative money amount is displayed wrongly", &workspace)
            .await
            .unwrap();

        assert!(
            briefing
                .text
                .contains("Lessons from earlier tasks that may apply:")
        );
        assert!(
            briefing
                .text
                .contains("learned from a run that failed, seen 2 times")
        );
        assert_eq!(briefing.detail["lessons"][0]["title"], "Money formatting");

        // An unrelated task is not shown it.
        assert!(memory.brief("Add a dark theme", &workspace).await.is_none());

        fs::remove_file(dir.path().join("money.ts")).unwrap();

        assert!(
            memory
                .brief("A negative money amount is displayed wrongly", &workspace)
                .await
                .is_none()
        );
        assert!(memory.store().lessons("repo").unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_finished_task_is_recalled_for_a_related_one() {
        let memory = system();
        let (_dir, workspace) = workspace_with(&["calendar.ts", "money.ts"]);

        assert!(memory.brief("anything", &workspace).await.is_none());

        play_run(
            &memory,
            "the calendar month view drops its last week",
            "calendar.ts",
            Some(true),
        );
        play_run(
            &memory,
            "commission rounding drifts by a cent",
            "money.ts",
            Some(true),
        );

        let briefing = memory
            .brief(
                "calendar shows evening trades on the wrong day of the month",
                &workspace,
            )
            .await
            .unwrap();

        // The related task is shown, the unrelated one is not.
        assert!(briefing.text.contains("drops its last week"));
        assert!(briefing.text.contains("Changed: calendar.ts"));
        assert!(!briefing.text.contains("commission rounding"));

        // The check the gate saw pass is a fact after the first task.
        assert!(
            briefing
                .text
                .contains("`CI=1 npm run test` is the project's check")
        );

        assert_eq!(briefing.detail["method"], "lexical");
        assert_eq!(memory.store().recall_count("repo").unwrap(), 1);
    }

    #[tokio::test]
    async fn a_task_in_another_project_is_recalled_without_its_files_or_facts() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("memory.db");

        {
            let shop = MemorySystem::open(&file, MemoryConfig::for_scope("/work/shop")).unwrap();

            play_run(
                &shop,
                "the calendar month view drops its last week",
                "calendar.ts",
                Some(true),
            );
            play_run(
                &shop,
                "commission rounding drifts by a cent",
                "money.ts",
                Some(true),
            );
        }

        let (_dir, workspace) = workspace_with(&["calendar.ts"]);

        // On its own this project remembers nothing.
        assert!(system().brief("anything", &workspace).await.is_none());

        let memory = system()
            .with_other(Store::open(&file).unwrap(), "/work/shop", "shop")
            // The project itself, listed among the others, adds nothing.
            .with_other(Store::open(&file).unwrap(), "repo", "repo");

        assert_eq!(memory.others(), 1);

        let briefing = memory
            .brief(
                "calendar shows evening trades on the wrong day of the month",
                &workspace,
            )
            .await
            .unwrap();

        assert!(briefing.text.contains("From your other projects"));
        assert!(
            briefing
                .text
                .contains("In `shop`: \"the calendar month view")
        );
        assert!(!briefing.text.contains("commission rounding"));

        // A file of the same name here is not the one that was changed
        // there, and that project's check is not this project's.
        assert!(!briefing.text.contains("Changed:"));
        assert!(!briefing.text.contains("is the project's check"));

        assert_eq!(briefing.detail["elsewhere"][0]["project"], "shop");

        // Nothing was written to the other project's memory.
        assert_eq!(
            Store::open(&file)
                .unwrap()
                .recall_count("/work/shop")
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn memories_of_deleted_files_are_dropped() {
        let memory = system();
        let (dir, workspace) = workspace_with(&["calendar.ts"]);

        play_run(&memory, "fix the calendar week grid", "calendar.ts", None);
        play_run(&memory, "fix the calendar day labels", "calendar.ts", None);

        memory
            .remember(
                "calendar.ts builds weeks Monday first",
                vec!["calendar.ts".to_string()],
            )
            .unwrap();

        let before = memory
            .recall("calendar weeks", workspace.root())
            .await
            .unwrap();

        assert!(
            before
                .facts
                .iter()
                .any(|fact| fact.kind == FactKind::HotFile)
        );
        assert!(before.facts.iter().any(|fact| fact.kind == FactKind::Note));

        fs::remove_file(dir.path().join("calendar.ts")).unwrap();

        let after = memory
            .recall("calendar weeks", workspace.root())
            .await
            .unwrap();

        assert!(after.facts.iter().all(|fact| fact.paths.is_empty()));
        assert!(
            !after
                .render(|path| workspace.root().join(path).exists())
                .contains("Changed:")
        );
    }

    #[tokio::test]
    async fn an_identical_task_can_be_left_out() {
        let config = MemoryConfig {
            skip_identical: true,
            ..MemoryConfig::for_scope("repo")
        };

        let memory = MemorySystem::new(Store::in_memory().unwrap(), config);
        let (_dir, workspace) = workspace_with(&["calendar.ts"]);

        play_run(&memory, "fix the calendar week grid", "calendar.ts", None);

        let recalled = memory
            .recall("fix the calendar week grid", workspace.root())
            .await
            .unwrap();

        assert!(recalled.episodes.is_empty());
    }

    struct FixedEmbedder;

    #[async_trait]
    impl Embedder for FixedEmbedder {
        async fn embed_query(&self, _text: &str) -> Option<Vec<f32>> {
            Some(vec![1.0, 0.0])
        }

        /// Texts about the calendar point the same way as the query.
        async fn embed_documents(&self, texts: &[String]) -> Option<Vec<Vec<f32>>> {
            let vectors = texts
                .iter()
                .map(|text| {
                    if text.contains("calendar") {
                        vec![1.0, 0.0]
                    } else {
                        vec![0.0, 1.0]
                    }
                })
                .collect();

            Some(vectors)
        }
    }

    /// Records what it is told and leaves the ranking alone.
    #[derive(Default)]
    struct WatchingReranker {
        observed: Mutex<Vec<(Vec<f32>, Vec<f32>)>>,
    }

    #[async_trait]
    impl Reranker for WatchingReranker {
        async fn rerank(&self, _query: &[f32], _candidates: &[Vec<f32>]) -> Option<Vec<f32>> {
            None
        }

        async fn observe(&self, query: &[f32], useful: &[f32]) {
            self.observed
                .lock()
                .unwrap()
                .push((query.to_vec(), useful.to_vec()));
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_reranker_hears_which_recalled_memory_was_useful() {
        let reranker = Arc::new(WatchingReranker::default());

        let memory = system()
            .with_embedder(Arc::new(FixedEmbedder))
            .with_reranker(reranker.clone());

        let (_dir, workspace) = workspace_with(&["calendar.ts", "money.ts"]);

        play_run(&memory, "calendar week grid", "calendar.ts", None);

        // A second calendar task: the first is recalled, and the run then
        // changes the same file, so the memory was useful.
        memory.record(&Event::RunStarted {
            instruction: "calendar day labels".to_string(),
            provider: "scripted".to_string(),
            model: "scripted".to_string(),
        });

        assert!(
            memory
                .brief("calendar day labels", &workspace)
                .await
                .is_some()
        );

        play_tools_and_finish(&memory, "calendar.ts", Some(true));

        let observed = reranker.observed.lock().unwrap();

        assert_eq!(observed.len(), 1);
        assert_eq!(observed[0], (vec![1.0, 0.0], vec![1.0, 0.0]));
    }

    /// Scores every memory far below the similarity floor, the newest
    /// highest.
    struct LowScoringReranker;

    #[async_trait]
    impl Reranker for LowScoringReranker {
        async fn rerank(&self, _query: &[f32], candidates: &[Vec<f32>]) -> Option<Vec<f32>> {
            Some(
                (0..candidates.len())
                    .map(|position| 0.01 * (position + 1) as f32)
                    .collect(),
            )
        }

        async fn observe(&self, _query: &[f32], _useful: &[f32]) {}
    }

    #[tokio::test]
    async fn a_reranker_changes_the_order_and_not_what_is_near_enough() {
        let plain = system().with_embedder(Arc::new(FixedEmbedder));
        let (_dir, workspace) = workspace_with(&["calendar.ts", "money.ts"]);

        play_run(&plain, "calendar week grid", "calendar.ts", None);
        play_run(&plain, "calendar day labels", "calendar.ts", None);
        play_run(&plain, "rounding of fees", "money.ts", None);

        let before = plain.recall("zzz", workspace.root()).await.unwrap();

        let instructions = |recall: &Recall| -> Vec<String> {
            recall
                .episodes
                .iter()
                .map(|scored| scored.episode.instruction.clone())
                .collect()
        };

        assert!(!before.reranked);
        assert_eq!(before.episodes.len(), 2);

        let reranked = plain.with_reranker(Arc::new(LowScoringReranker));
        let after = reranked.recall("zzz", workspace.root()).await.unwrap();

        // The same two memories, although their new scores are under the
        // floor, and the unrelated one is still left out.
        assert!(after.reranked);
        assert_eq!(after.episodes.len(), 2);
        assert!(!instructions(&after).contains(&"rounding of fees".to_string()));

        let mut same = instructions(&before);
        same.reverse();

        assert_eq!(instructions(&after), same);
    }

    #[tokio::test]
    async fn embeddings_are_used_and_saved_when_available() {
        let memory = system().with_embedder(Arc::new(FixedEmbedder));
        let (_dir, workspace) = workspace_with(&["calendar.ts", "money.ts"]);

        play_run(&memory, "calendar week grid", "calendar.ts", None);
        play_run(&memory, "rounding of fees", "money.ts", None);

        // The query shares no word with either task; only the vectors match.
        let recalled = memory.recall("zzz", workspace.root()).await.unwrap();

        assert_eq!(recalled.method, "embedding");
        assert_eq!(recalled.episodes.len(), 1);
        assert_eq!(
            recalled.episodes[0].episode.instruction,
            "calendar week grid"
        );

        let stored = memory.store().episodes("repo").unwrap();

        assert!(stored.iter().all(|stored| stored.embedding.is_some()));
    }
}
