//! The trajectory log of the current session.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_LOG: AtomicU64 = AtomicU64::new(0);

use cortex_core::trajectory::unix_millis;
use cortex_core::{Event, JsonlSink, TrajectorySink};

use super::{Outbox, Update};
use crate::session::history::TRAJECTORY_DIRECTORY;

/// Writes each session to its own file, in the same place and format as
/// the command-line agent. The file is created on the session's first
/// event, so a session in which nothing was asked leaves no empty log.
pub struct SessionLog {
    brain_session: Mutex<String>,
    directory: PathBuf,
    current: Mutex<Option<JsonlSink>>,

    /// The file being written to, and who is told its name: the records
    /// of tokens and runs, which file what they hold under it.
    path: Mutex<Option<PathBuf>>,
    namer: Mutex<Option<Namer>>,

    /// Told which file a new session is written to, so the window knows
    /// that file is the session it is showing.
    outbox: Outbox,
}

/// Told the name of each session's log as it begins.
pub type Namer = Box<dyn Fn(&str) + Send + Sync>;

impl SessionLog {
    pub fn new(project: &Path, outbox: Outbox) -> Self {
        Self {
            brain_session: Mutex::new(new_session_key()),
            directory: project.join(TRAJECTORY_DIRECTORY),
            current: Mutex::new(None),
            path: Mutex::new(None),
            namer: Mutex::new(None),
            outbox,
        }
    }

    /// Who is told which session is being written. Told at once if one
    /// already is: the agent is rebuilt in the middle of sessions.
    pub fn name_with(&self, namer: Namer) {
        if let Some(path) = &*lock(&self.path) {
            namer(&stem(path));
        }

        *lock(&self.namer) = Some(namer);
    }

    fn began(&self, path: &Path) {
        *lock(&self.brain_session) = stem(path);
        *lock(&self.path) = Some(path.to_path_buf());

        if let Some(namer) = &*lock(&self.namer) {
            namer(&stem(path));
        }
    }

    /// Goes on writing to the log of an earlier session.
    pub fn resume(&self, path: &Path) {
        match JsonlSink::append(path) {
            Ok(sink) => {
                *self.lock() = Some(sink);

                self.began(path);
            }

            // The session carries on in a log of its own.
            Err(error) => {
                eprintln!("warning: cannot add to {}: {error}", path.display());

                self.start_new_session();
            }
        }
    }

    /// Ends the current file; the next event starts a new one.
    pub fn start_new_session(&self) {
        *self.lock() = None;
        *lock(&self.path) = None;
        *lock(&self.brain_session) = new_session_key();
    }

    pub fn brain_session(&self) -> String {
        format!("desktop-{}", &*lock(&self.brain_session))
    }

    fn create_log(&self) -> std::io::Result<(PathBuf, JsonlSink)> {
        std::fs::create_dir_all(&self.directory)?;
        loop {
            let path = self
                .directory
                .join(format!("{}.jsonl", &*lock(&self.brain_session)));
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(_) => return JsonlSink::append(&path).map(|sink| (path, sink)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    *lock(&self.brain_session) = new_session_key();
                    continue;
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<JsonlSink>> {
        self.current
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn new_session_key() -> String {
    format!(
        "{}-{}-{}",
        unix_millis(),
        std::process::id(),
        NEXT_LOG.fetch_add(1, Ordering::Relaxed)
    )
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl TrajectorySink for SessionLog {
    fn record(&self, event: &Event) {
        let mut current = self.lock();

        if current.is_none() {
            match self.create_log() {
                Ok((path, sink)) => {
                    *current = Some(sink);

                    self.began(&path);
                    self.outbox.send(Update::Logging(path));
                }

                Err(error) => {
                    eprintln!(
                        "warning: cannot create a session log in {}: {error}",
                        self.directory.display()
                    );

                    return;
                }
            }
        }

        if let Some(sink) = current.as_ref() {
            sink.record(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier, mpsc};

    #[test]
    fn concurrent_sessions_create_distinct_logs_without_overwriting() {
        let folder = tempfile::tempdir().unwrap();
        let (updates, receiver) = mpsc::channel();
        let outbox = Outbox {
            updates,
            window: eframe::egui::Context::default(),
            chat: "agent-1".into(),
        };
        let barrier = Arc::new(Barrier::new(32));
        std::thread::scope(|scope| {
            for index in 0..32 {
                let barrier = barrier.clone();
                let log = SessionLog::new(folder.path(), outbox.clone());
                scope.spawn(move || {
                    barrier.wait();
                    log.record(&Event::RunStarted {
                        instruction: format!("task {index}"),
                        provider: "scripted".into(),
                        model: "scripted".into(),
                    });
                });
            }
        });
        let paths: std::collections::BTreeSet<_> = receiver
            .try_iter()
            .filter_map(|update| match update {
                Update::Logging(path) => Some(path),
                _ => None,
            })
            .collect();
        assert_eq!(paths.len(), 32);
        let mut instructions = std::collections::BTreeSet::new();
        for path in paths {
            let content = std::fs::read_to_string(path).unwrap();
            assert_eq!(content.lines().count(), 1);
            let line: serde_json::Value = serde_json::from_str(&content).unwrap();
            instructions.insert(line["instruction"].as_str().unwrap().to_string());
        }
        assert_eq!(instructions.len(), 32);
    }
}
