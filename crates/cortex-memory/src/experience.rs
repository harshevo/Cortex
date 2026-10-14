use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::store::MemoryError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Domain {
    Trading,
    Coding,
    General,
}

impl Domain {
    pub fn label(self) -> &'static str {
        match self {
            Self::Trading => "trading",
            Self::Coding => "coding",
            Self::General => "general",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Experience {
    pub id: i64,
    pub key: String,
    pub domain: Domain,
    pub scope: String,
    pub session: String,
    pub kind: String,
    pub created_ms: i64,
    pub available_ms: i64,
    pub text: String,
    pub evidence: Value,
    pub verified: bool,
}

pub struct ExperienceStore {
    connection: Mutex<Connection>,
}

impl ExperienceStore {
    pub fn open(path: &Path) -> Result<Self, MemoryError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path)?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS experiences (
                id INTEGER PRIMARY KEY, event_key TEXT NOT NULL UNIQUE,
                domain TEXT NOT NULL, scope TEXT NOT NULL, session TEXT NOT NULL,
                kind TEXT NOT NULL, created_ms INTEGER NOT NULL,
                available_ms INTEGER NOT NULL, text TEXT NOT NULL,
                evidence TEXT NOT NULL, verified INTEGER NOT NULL);
             CREATE INDEX IF NOT EXISTS experience_context ON experiences(domain, scope, available_ms);
             CREATE INDEX IF NOT EXISTS experience_session ON experiences(session, id);",
        )?;
        let indexed: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='experiences_fts')",
            [],
            |row| row.get(0),
        )?;
        connection.execute_batch("CREATE VIRTUAL TABLE IF NOT EXISTS experiences_fts USING fts5(text,scope,kind,content='experiences',content_rowid='id');
            CREATE TRIGGER IF NOT EXISTS experiences_ai AFTER INSERT ON experiences BEGIN
                INSERT INTO experiences_fts(rowid,text,scope,kind) VALUES (new.id,new.text,new.scope,new.kind); END;
            CREATE TRIGGER IF NOT EXISTS experiences_ad AFTER DELETE ON experiences BEGIN
                INSERT INTO experiences_fts(experiences_fts,rowid,text,scope,kind) VALUES ('delete',old.id,old.text,old.scope,old.kind); END;
            CREATE TRIGGER IF NOT EXISTS experiences_au AFTER UPDATE ON experiences BEGIN
                INSERT INTO experiences_fts(experiences_fts,rowid,text,scope,kind) VALUES ('delete',old.id,old.text,old.scope,old.kind);
                INSERT INTO experiences_fts(rowid,text,scope,kind) VALUES (new.id,new.text,new.scope,new.kind); END;")?;
        if !indexed {
            connection.execute(
                "INSERT INTO experiences_fts(experiences_fts) VALUES('rebuild')",
                [],
            )?;
        }
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    fn connection(&self) -> MutexGuard<'_, Connection> {
        self.connection.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn append(&self, event: &Experience) -> Result<i64, MemoryError> {
        let connection = self.connection();
        connection.execute(
            "INSERT OR IGNORE INTO experiences(event_key,domain,scope,session,kind,created_ms,available_ms,text,evidence,verified)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![event.key, event.domain.label(), event.scope, event.session, event.kind,
                event.created_ms, event.available_ms, event.text,
                serde_json::to_string(&event.evidence)?, event.verified],
        )?;
        Ok(connection.query_row(
            "SELECT id FROM experiences WHERE event_key=?1",
            [&event.key],
            |r| r.get(0),
        )?)
    }

    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Experience> {
        let domain: String = row.get(2)?;
        let evidence: String = row.get(9)?;
        let evidence = serde_json::from_str(&evidence).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                9,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;
        Ok(Experience {
            id: row.get(0)?,
            key: row.get(1)?,
            domain: match domain.as_str() {
                "trading" => Domain::Trading,
                "coding" => Domain::Coding,
                _ => Domain::General,
            },
            scope: row.get(3)?,
            session: row.get(4)?,
            kind: row.get(5)?,
            created_ms: row.get(6)?,
            available_ms: row.get(7)?,
            text: row.get(8)?,
            evidence,
            verified: row.get(10)?,
        })
    }

    pub fn session(&self, session: &str, limit: usize) -> Result<Vec<Experience>, MemoryError> {
        let connection = self.connection();
        let mut query = connection.prepare("SELECT * FROM experiences WHERE session=?1 AND kind IN ('user','assistant','cancelled','error') ORDER BY id DESC LIMIT ?2")?;
        let mut events = query
            .query_map(params![session, limit.min(1000) as i64], Self::read)?
            .collect::<Result<Vec<_>, _>>()?;
        events.reverse();
        Ok(events)
    }

    pub fn sessions(&self) -> Result<Vec<String>, MemoryError> {
        let connection = self.connection();
        let mut query = connection.prepare("SELECT session FROM experiences WHERE kind='user' GROUP BY session ORDER BY MAX(id) DESC LIMIT 100")?;
        Ok(query
            .query_map([], |r| r.get(0))?
            .collect::<Result<Vec<_>, _>>()?)
    }
    pub fn get(
        &self,
        id: i64,
        domain: Domain,
        as_of: i64,
    ) -> Result<Option<Experience>, MemoryError> {
        use rusqlite::OptionalExtension;
        Ok(self.connection().query_row("SELECT * FROM experiences WHERE id=?1 AND available_ms<=?2 AND (domain=?3 OR (domain='general' AND verified=1))",params![id,as_of,domain.label()],Self::read).optional()?)
    }

    pub fn recall(
        &self,
        domain: Domain,
        scope: &str,
        text: &str,
        as_of: i64,
        limit: usize,
    ) -> Result<Vec<Experience>, MemoryError> {
        let connection = self.connection();
        let words: Vec<_> = text
            .split(|c: char| !c.is_alphanumeric())
            .filter(|word| word.len() > 2)
            .take(32)
            .map(|word| format!("\"{}\"*", word.to_lowercase()))
            .collect();
        let limit = limit.min(50) as i64;
        if words.is_empty() {
            let mut query=connection.prepare("SELECT * FROM experiences WHERE available_ms<=?1 AND ((domain=?2 AND (scope=?3 OR scope='*' OR ?3='')) OR (domain='general' AND verified=1)) ORDER BY id DESC LIMIT ?4")?;
            return Ok(query
                .query_map(params![as_of, domain.label(), scope, limit], Self::read)?
                .collect::<Result<Vec<_>, _>>()?);
        }
        let mut query=connection.prepare("SELECT e.* FROM experiences_fts JOIN experiences e ON e.id=experiences_fts.rowid
            WHERE experiences_fts MATCH ?4 AND available_ms<=?1 AND ((domain=?2 AND (e.scope=?3 OR e.scope='*' OR ?3='')) OR (domain='general' AND verified=1))
            ORDER BY bm25(experiences_fts),e.id DESC LIMIT ?5")?;
        Ok(query
            .query_map(
                params![as_of, domain.label(), scope, words.join(" OR "), limit],
                Self::read,
            )?
            .collect::<Result<Vec<_>, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn durable_idempotent_scoped_point_in_time_recall() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("memory.db");
        let store = ExperienceStore::open(&file).unwrap();
        let mut event = Experience {
            id: 0,
            key: "one".into(),
            domain: Domain::Trading,
            scope: "NSE_EQ:13".into(),
            session: "chat".into(),
            kind: "user".into(),
            created_ms: 1,
            available_ms: 10,
            text: "liquidity lesson".into(),
            evidence: Value::Null,
            verified: false,
        };
        let id = store.append(&event).unwrap();
        assert_eq!(store.append(&event).unwrap(), id);
        event.key = "coding".into();
        event.domain = Domain::Coding;
        store.append(&event).unwrap();
        event.key = "future".into();
        event.domain = Domain::Trading;
        event.available_ms = 30;
        store.append(&event).unwrap();
        assert_eq!(
            store
                .recall(Domain::Trading, "NSE_EQ:13", "liquidity", 20, 10)
                .unwrap()
                .len(),
            1
        );
        assert!(
            store
                .recall(Domain::Trading, "IDX_I:13", "liquidity", 20, 10)
                .unwrap()
                .is_empty()
        );
        drop(store);
        assert_eq!(
            ExperienceStore::open(&file)
                .unwrap()
                .session("chat", 10)
                .unwrap()
                .len(),
            3
        );
    }
    #[test]
    fn search_reaches_old_memories_without_loading_all_rows() {
        let dir = tempfile::tempdir().unwrap();
        let store = ExperienceStore::open(&dir.path().join("memory.db")).unwrap();
        let mut event = Experience {
            id: 0,
            key: "old".into(),
            domain: Domain::Trading,
            scope: "symbol".into(),
            session: "old-chat".into(),
            kind: "user".into(),
            created_ms: 1,
            available_ms: 1,
            text: "distinctive liquidity reversal".into(),
            evidence: Value::Null,
            verified: false,
        };
        let old = store.append(&event).unwrap();
        for at in 0..1600 {
            event.key = at.to_string();
            event.text = "unrelated noise".into();
            store.append(&event).unwrap();
        }
        let events = store
            .recall(Domain::Trading, "symbol", "distinctive", 2, 5)
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, old);
    }
}
