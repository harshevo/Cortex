//! Where memories are kept: one SQLite file.
//!
//! Each memory is stored as a JSON document beside the few columns that
//! are queried, so adding a field to an episode needs no migration.
//!
//! A `scope` separates repositories that share a file. Memories from one
//! scope are never shown in another.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use rusqlite::{Connection, params};
use serde_json::Value;
use thiserror::Error;

use crate::memory::episode::Episode;
use crate::memory::fact::{Fact, FactKind};
use crate::memory::lesson::StoredLesson;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS episodes (
    id          INTEGER PRIMARY KEY,
    scope       TEXT    NOT NULL,
    created_ms  INTEGER NOT NULL,
    body        TEXT    NOT NULL,
    embedding   BLOB
);

CREATE INDEX IF NOT EXISTS episodes_by_scope ON episodes (scope, created_ms);

CREATE TABLE IF NOT EXISTS facts (
    scope  TEXT NOT NULL,
    kind   TEXT NOT NULL,
    key    TEXT NOT NULL,
    body   TEXT NOT NULL,

    PRIMARY KEY (scope, kind, key)
);

CREATE TABLE IF NOT EXISTS lessons (
    id          INTEGER PRIMARY KEY,
    scope       TEXT    NOT NULL,
    created_ms  INTEGER NOT NULL,
    body        TEXT    NOT NULL,
    embedding   BLOB
);

CREATE INDEX IF NOT EXISTS lessons_by_scope ON lessons (scope, created_ms);

CREATE TABLE IF NOT EXISTS recalls (
    id          INTEGER PRIMARY KEY,
    scope       TEXT    NOT NULL,
    created_ms  INTEGER NOT NULL,
    query       TEXT    NOT NULL,
    detail      TEXT    NOT NULL
);
";

#[derive(Debug, Error)]
pub enum MemoryError {
    #[error("the memory store failed: {0}")]
    Store(#[from] rusqlite::Error),

    #[error("a stored memory could not be read or written: {0}")]
    Encoding(#[from] serde_json::Error),

    #[error("could not create the memory directory: {0}")]
    Directory(#[from] std::io::Error),
}

/// An episode together with its vector, when it has been embedded.
#[derive(Debug, Clone)]
pub struct StoredEpisode {
    pub episode: Episode,
    pub embedding: Option<Vec<f32>>,
}

/// A lesson together with its vector, when it has been embedded.
#[derive(Debug, Clone)]
pub struct LessonRow {
    pub lesson: StoredLesson,
    pub embedding: Option<Vec<f32>>,
}

static STORE_INITIALIZATION: Mutex<()> = Mutex::new(());

pub struct Store {
    connection: Mutex<Connection>,
}

impl Store {
    pub fn consolidate_episode(
        &self,
        scope: &str,
        episode: &Episode,
        now: u64,
    ) -> Result<(), MemoryError> {
        let mut connection = self.connection();
        let transaction =
            connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        transaction.execute(
            "INSERT INTO episodes (scope, created_ms, body) VALUES (?1, ?2, ?3)",
            params![
                scope,
                episode.created_ms as i64,
                serde_json::to_string(episode)?
            ],
        )?;
        let episodes = {
            let mut statement = transaction
                .prepare("SELECT body FROM episodes WHERE scope = ?1 ORDER BY created_ms, id")?;
            let rows = statement.query_map(params![scope], |row| row.get::<_, String>(0))?;
            let mut episodes = Vec::new();
            for row in rows {
                episodes.push(serde_json::from_str::<Episode>(&row?)?);
            }
            episodes
        };
        transaction.execute(
            "DELETE FROM facts WHERE scope = ?1 AND kind NOT IN (?2, ?3)",
            params![scope, FactKind::Note.label(), FactKind::Overview.label()],
        )?;
        for fact in crate::memory::consolidate::derive_facts(&episodes, now) {
            transaction.execute(
                "INSERT OR REPLACE INTO facts (scope, kind, key, body) VALUES (?1, ?2, ?3, ?4)",
                params![
                    scope,
                    fact.kind.label(),
                    fact.key,
                    serde_json::to_string(&fact)?
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }
    pub fn open(path: &Path) -> Result<Self, MemoryError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        Self::prepare(Connection::open(path)?)
    }

    pub fn in_memory() -> Result<Self, MemoryError> {
        Self::prepare(Connection::open_in_memory()?)
    }

    fn prepare(connection: Connection) -> Result<Self, MemoryError> {
        let _initialization = STORE_INITIALIZATION
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        // WAL lets a second process read while this one writes. It is not
        // available for in-memory databases, which report that as a value
        // rather than an error.
        connection.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;

        connection.execute_batch(SCHEMA)?;

        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    fn connection(&self) -> MutexGuard<'_, Connection> {
        self.connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Saves an episode and returns its id.
    pub fn add_episode(&self, scope: &str, episode: &Episode) -> Result<i64, MemoryError> {
        let body = serde_json::to_string(episode)?;

        let connection = self.connection();

        connection.execute(
            "INSERT INTO episodes (scope, created_ms, body) VALUES (?1, ?2, ?3)",
            params![scope, episode.created_ms as i64, body],
        )?;

        Ok(connection.last_insert_rowid())
    }

    /// Every episode in `scope`, oldest first.
    pub fn episodes(&self, scope: &str) -> Result<Vec<StoredEpisode>, MemoryError> {
        self.episode_rows(scope, None)
    }

    pub fn episode_count(&self, scope: &str) -> Result<usize, MemoryError> {
        Ok(self.connection().query_row(
            "SELECT COUNT(*) FROM episodes WHERE scope = ?1",
            params![scope],
            |row| row.get::<_, i64>(0),
        )? as usize)
    }

    pub fn recent_episodes(
        &self,
        scope: &str,
        limit: usize,
    ) -> Result<Vec<StoredEpisode>, MemoryError> {
        let mut episodes = self.episode_rows(scope, Some(limit))?;
        episodes.reverse();
        Ok(episodes)
    }

    fn episode_rows(
        &self,
        scope: &str,
        limit: Option<usize>,
    ) -> Result<Vec<StoredEpisode>, MemoryError> {
        let connection = self.connection();

        let sql = match limit {
            Some(_) => {
                "SELECT id, body, embedding FROM episodes WHERE scope = ?1 ORDER BY created_ms DESC, id DESC LIMIT ?2"
            }
            None => {
                "SELECT id, body, embedding FROM episodes WHERE scope = ?1 ORDER BY created_ms, id LIMIT ?2"
            }
        };
        let mut statement = connection.prepare(sql)?;

        let rows = statement.query_map(
            params![scope, limit.map(|limit| limit as i64).unwrap_or(-1)],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<Vec<u8>>>(2)?,
                ))
            },
        )?;

        let mut episodes = Vec::new();

        for row in rows {
            let (id, body, embedding) = row?;

            let mut episode: Episode = serde_json::from_str(&body)?;

            episode.id = id;

            episodes.push(StoredEpisode {
                episode,
                embedding: embedding.as_deref().map(decode_vector),
            });
        }

        Ok(episodes)
    }

    pub fn set_episode_embedding(&self, id: i64, vector: &[f32]) -> Result<(), MemoryError> {
        self.connection().execute(
            "UPDATE episodes SET embedding = ?1 WHERE id = ?2",
            params![encode_vector(vector), id],
        )?;

        Ok(())
    }

    /// Adds a fact, or replaces the one with the same kind and key.
    pub fn put_fact(&self, scope: &str, fact: &Fact) -> Result<(), MemoryError> {
        let body = serde_json::to_string(fact)?;

        self.connection().execute(
            "INSERT INTO facts (scope, kind, key, body) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (scope, kind, key) DO UPDATE SET body = excluded.body",
            params![scope, fact.kind.label(), fact.key, body],
        )?;

        Ok(())
    }

    /// Replaces every fact in `scope` that was derived from episodes.
    /// Notes written by the agent and the project's overview are kept.
    pub fn replace_derived_facts(&self, scope: &str, facts: &[Fact]) -> Result<(), MemoryError> {
        let mut connection = self.connection();

        let transaction = connection.transaction()?;

        transaction.execute(
            "DELETE FROM facts WHERE scope = ?1 AND kind NOT IN (?2, ?3)",
            params![scope, FactKind::Note.label(), FactKind::Overview.label()],
        )?;

        for fact in facts {
            transaction.execute(
                "INSERT OR REPLACE INTO facts (scope, kind, key, body) VALUES (?1, ?2, ?3, ?4)",
                params![
                    scope,
                    fact.kind.label(),
                    fact.key,
                    serde_json::to_string(fact)?
                ],
            )?;
        }

        transaction.commit()?;

        Ok(())
    }

    pub fn facts(&self, scope: &str) -> Result<Vec<Fact>, MemoryError> {
        let connection = self.connection();

        let mut statement = connection.prepare("SELECT body FROM facts WHERE scope = ?1")?;

        let rows = statement.query_map(params![scope], |row| row.get::<_, String>(0))?;

        let mut facts = Vec::new();

        for row in rows {
            facts.push(serde_json::from_str(&row?)?);
        }

        Ok(facts)
    }

    pub fn delete_fact(&self, scope: &str, fact: &Fact) -> Result<(), MemoryError> {
        self.connection().execute(
            "DELETE FROM facts WHERE scope = ?1 AND kind = ?2 AND key = ?3",
            params![scope, fact.kind.label(), fact.key],
        )?;

        Ok(())
    }

    /// Saves a new lesson and returns its id.
    pub fn add_lesson(&self, scope: &str, lesson: &StoredLesson) -> Result<i64, MemoryError> {
        let body = serde_json::to_string(lesson)?;

        let connection = self.connection();

        connection.execute(
            "INSERT INTO lessons (scope, created_ms, body) VALUES (?1, ?2, ?3)",
            params![scope, lesson.created_ms as i64, body],
        )?;

        Ok(connection.last_insert_rowid())
    }

    /// Writes a changed lesson back. Its vector is kept.
    pub fn update_lesson(&self, lesson: &StoredLesson) -> Result<(), MemoryError> {
        self.connection().execute(
            "UPDATE lessons SET body = ?1 WHERE id = ?2",
            params![serde_json::to_string(lesson)?, lesson.id],
        )?;

        Ok(())
    }

    /// Every lesson in `scope`, oldest first.
    pub fn lessons(&self, scope: &str) -> Result<Vec<LessonRow>, MemoryError> {
        let connection = self.connection();

        let mut statement = connection.prepare(
            "SELECT id, body, embedding FROM lessons WHERE scope = ?1 ORDER BY created_ms, id",
        )?;

        let rows = statement.query_map(params![scope], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<Vec<u8>>>(2)?,
            ))
        })?;

        let mut lessons = Vec::new();

        for row in rows {
            let (id, body, embedding) = row?;

            let mut lesson: StoredLesson = serde_json::from_str(&body)?;

            lesson.id = id;

            lessons.push(LessonRow {
                lesson,
                embedding: embedding.as_deref().map(decode_vector),
            });
        }

        Ok(lessons)
    }

    pub fn set_lesson_embedding(&self, id: i64, vector: &[f32]) -> Result<(), MemoryError> {
        self.connection().execute(
            "UPDATE lessons SET embedding = ?1 WHERE id = ?2",
            params![encode_vector(vector), id],
        )?;

        Ok(())
    }

    pub fn delete_lesson(&self, id: i64) -> Result<(), MemoryError> {
        self.connection()
            .execute("DELETE FROM lessons WHERE id = ?1", params![id])?;

        Ok(())
    }

    /// Records what was shown for a task. Together with how the task then
    /// went, this is the training data for a learned retriever.
    pub fn log_recall(
        &self,
        scope: &str,
        created_ms: u64,
        query: &str,
        detail: &Value,
    ) -> Result<(), MemoryError> {
        self.connection().execute(
            "INSERT INTO recalls (scope, created_ms, query, detail) VALUES (?1, ?2, ?3, ?4)",
            params![scope, created_ms as i64, query, detail.to_string()],
        )?;

        Ok(())
    }

    pub fn recall_count(&self, scope: &str) -> Result<u64, MemoryError> {
        let count: i64 = self.connection().query_row(
            "SELECT COUNT(*) FROM recalls WHERE scope = ?1",
            params![scope],
            |row| row.get(0),
        )?;

        Ok(count as u64)
    }
}

fn encode_vector(vector: &[f32]) -> Vec<u8> {
    vector
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn decode_vector(bytes: &[u8]) -> Vec<f32> {
    let (values, _) = bytes.as_chunks::<4>();

    values
        .iter()
        .map(|value| f32::from_le_bytes(*value))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::working::WorkingMemory;

    fn episode(instruction: &str) -> Episode {
        let memory = WorkingMemory {
            instruction: instruction.to_string(),
            files_changed: vec!["a.ts".to_string()],
            ..WorkingMemory::default()
        };

        Episode::from_working(&memory, 5)
    }

    #[test]
    fn episodes_round_trip_and_stay_in_their_scope() {
        let store = Store::in_memory().unwrap();

        let id = store.add_episode("repo-a", &episode("fix a")).unwrap();

        store.add_episode("repo-b", &episode("fix b")).unwrap();
        store.set_episode_embedding(id, &[0.25, -1.0]).unwrap();

        let found = store.episodes("repo-a").unwrap();

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].episode.id, id);
        assert_eq!(found[0].episode.instruction, "fix a");
        assert_eq!(found[0].embedding.as_deref(), Some(&[0.25, -1.0][..]));

        assert!(store.episodes("repo-b").unwrap()[0].embedding.is_none());
    }

    #[test]
    fn derived_facts_are_replaced_and_notes_are_kept() {
        let store = Store::in_memory().unwrap();

        let derived = |key: &str| Fact {
            kind: FactKind::HotFile,
            key: key.to_string(),
            text: key.to_string(),
            paths: Vec::new(),
            support: 2,
            updated_ms: 0,
        };

        store
            .put_fact("repo", &Fact::note("uses Decimal for money", Vec::new(), 0))
            .unwrap();
        store
            .replace_derived_facts("repo", &[derived("a.ts")])
            .unwrap();
        store
            .replace_derived_facts("repo", &[derived("b.ts")])
            .unwrap();

        let mut keys: Vec<String> = store
            .facts("repo")
            .unwrap()
            .into_iter()
            .map(|fact| fact.key)
            .collect();

        keys.sort();

        assert_eq!(keys, ["b.ts", "uses decimal for money"]);
    }

    #[test]
    fn concurrent_agents_can_open_and_write_the_same_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.db");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        std::thread::scope(|scope| {
            for index in 0..8 {
                let barrier = barrier.clone();
                let path = path.clone();
                scope.spawn(move || {
                    barrier.wait();
                    let store = Store::open(&path).unwrap();
                    store
                        .add_episode("team", &episode(&format!("task {index}")))
                        .unwrap();
                });
            }
        });
        assert_eq!(
            Store::open(&path).unwrap().episodes("team").unwrap().len(),
            8
        );
    }

    #[test]
    fn a_store_on_disk_survives_being_reopened() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory/memory.db");

        Store::open(&path)
            .unwrap()
            .add_episode("repo", &episode("fix a"))
            .unwrap();

        assert_eq!(
            Store::open(&path).unwrap().episodes("repo").unwrap().len(),
            1
        );
    }
}
