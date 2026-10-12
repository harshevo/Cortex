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
    directory: PathBuf,
    current: Mutex<Option<JsonlSink>>,

    /// Told which file a new session is written to, so the window knows
    /// that file is the session it is showing.
    outbox: Outbox,
}

impl SessionLog {
    pub fn new(project: &Path, outbox: Outbox) -> Self {
        Self {
            directory: project.join(TRAJECTORY_DIRECTORY),
            current: Mutex::new(None),
            outbox,
        }
    }

    /// Goes on writing to the log of an earlier session.
    pub fn resume(&self, path: &Path) {
        match JsonlSink::append(path) {
            Ok(sink) => *self.lock() = Some(sink),

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
    }

    fn create_log(&self) -> std::io::Result<(PathBuf, JsonlSink)> {
        std::fs::create_dir_all(&self.directory)?;
        loop {
            let counter = NEXT_LOG.fetch_add(1, Ordering::Relaxed);
            let path = self.directory.join(format!(
                "{}-{}-{counter}.jsonl",
                unix_millis(),
                std::process::id()
            ));
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(_) => return JsonlSink::append(&path).map(|sink| (path, sink)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
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

impl TrajectorySink for SessionLog {
    fn record(&self, event: &Event) {
        let mut current = self.lock();

        if current.is_none() {
            match self.create_log() {
                Ok((path, sink)) => {
                    *current = Some(sink);

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
