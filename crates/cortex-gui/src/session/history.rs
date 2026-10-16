//! Past sessions, read back from the trajectory logs.

use std::cmp::Reverse;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use cortex_core::Event;

use super::Session;

/// Folder inside the project where the engine logs every run.
pub const TRAJECTORY_DIRECTORY: &str = ".cortex/trajectories";

/// Past sessions listed in the sidebar.
const MAX_LISTED: usize = 40;

/// Most titles remembered before the list is started again.
const MAX_TITLES: usize = 4_096;

/// The first instruction of each log that has been listed. A log is
/// only ever added to, so its first instruction never changes, and the
/// sidebar's list is asked for many times in a session: on every run's
/// start and end, and whenever another chat is brought forward. With
/// this, asking again reads the folder and no file in it.
static TITLES: Mutex<Option<HashMap<PathBuf, String>>> = Mutex::new(None);

/// The first instruction in the log at `path`, read once.
fn title(path: &Path) -> Option<String> {
    let known = |titles: &mut Option<HashMap<PathBuf, String>>| {
        titles.get_or_insert_with(HashMap::new).get(path).cloned()
    };

    let mut titles = TITLES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    if let Some(title) = known(&mut titles) {
        return Some(title);
    }

    // Not held while the file is read.
    drop(titles);

    // A log with no run in it yet may gain one, so it is not remembered.
    let title = first_instruction(path)?;

    let mut titles = TITLES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let titles = titles.get_or_insert_with(HashMap::new);

    if titles.len() >= MAX_TITLES {
        titles.clear();
    }

    titles.insert(path.to_path_buf(), title.clone());

    Some(title)
}

fn forget_title(path: &Path) {
    let mut titles = TITLES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    if let Some(titles) = titles.as_mut() {
        titles.remove(path);
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PastSession {
    pub path: PathBuf,

    /// When the session started, in milliseconds since 1970.
    pub started_ms: u64,

    pub updated_ms: u64,

    /// The first instruction of the session.
    pub title: String,
}

/// The newest sessions in `project`, newest first. Logs that hold no run
/// are left out.
pub fn list(project: &Path) -> Vec<PastSession> {
    let Ok(entries) = fs::read_dir(project.join(TRAJECTORY_DIRECTORY)) else {
        return Vec::new();
    };

    let mut logs: Vec<(u64, u64, PathBuf)> = entries
        .filter_map(|entry| {
            let path = entry.ok()?.path();

            // The engine names each log after the moment it was created.
            let started_ms = path
                .file_stem()?
                .to_str()?
                .split('-')
                .next()?
                .parse()
                .ok()?;

            let updated_ms = fs::metadata(&path)
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(started_ms, |duration| duration.as_millis() as u64)
                .max(started_ms);
            Some((updated_ms, started_ms, path))
        })
        .collect();

    logs.sort_by_key(|(updated_ms, started_ms, _)| Reverse((*updated_ms, *started_ms)));

    logs.into_iter()
        .filter_map(|(updated_ms, started_ms, path)| {
            let title = title(&path)?;

            Some(PastSession {
                path,
                started_ms,
                updated_ms,
                title,
            })
        })
        .take(MAX_LISTED)
        .collect()
}

/// Deletes a session's log. The work the session did in the project is
/// not undone; only the record of the conversation goes.
pub fn delete(path: &Path) -> std::io::Result<()> {
    forget_title(path);

    fs::remove_file(path)
}

/// Streams a log into its display state, trimming each tool payload immediately.
pub fn load_cancelled(
    path: &Path,
    cancelled: &std::sync::atomic::AtomicBool,
) -> std::io::Result<Session> {
    let file = File::open(path)?;
    let mut session = Session::default();
    for line in BufReader::new(file).lines() {
        if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "session loading cancelled",
            ));
        }
        if let Ok(event) = serde_json::from_str::<Event>(&line?) {
            session.apply(&event);
            super::slim_after(&mut session, &event);
        }
    }
    session.running = false;
    session.stats.run_ended();
    Ok(session)
}

#[cfg(test)]
pub fn load(path: &Path) -> Session {
    load_cancelled(path, &std::sync::atomic::AtomicBool::new(false)).unwrap()
}

fn first_instruction(path: &Path) -> Option<String> {
    events(path).find_map(|event| match event {
        Event::RunStarted { instruction, .. } => Some(instruction),
        _ => None,
    })
}

/// Every event in a session's log, in order.
pub fn events(path: &Path) -> impl Iterator<Item = Event> {
    File::open(path)
        .ok()
        .into_iter()
        .flat_map(|file| BufReader::new(file).lines())
        .filter_map(|line| serde_json::from_str(&line.ok()?).ok())
}

#[cfg(test)]
mod tests {
    use cortex_core::{JsonlSink, Outcome, TrajectorySink, Usage};

    use super::*;
    use crate::session::Item;

    fn write_log(project: &Path, name: &str, instruction: &str) {
        let path = project.join(TRAJECTORY_DIRECTORY).join(name);
        let sink = JsonlSink::create(&path).unwrap();

        sink.record(&Event::RunStarted {
            instruction: instruction.to_string(),
            provider: "scripted".to_string(),
            model: "scripted".to_string(),
        });

        sink.record(&Event::RunFinished {
            outcome: Outcome::Completed,
            steps: 0,
            usage: Usage::default(),
            duration_ms: 5,
            verified: None,
        });
    }

    #[test]
    fn large_logs_are_slimmed_during_replay_and_can_be_cancelled() {
        use cortex_core::{ToolCall, ToolResult};
        let project = tempfile::tempdir().unwrap();
        let path = project.path().join("large.jsonl");
        let sink = JsonlSink::create(&path).unwrap();
        for number in 0..16 {
            let id = number.to_string();
            sink.record(&Event::ToolStarted {
                step: 1,
                call: ToolCall {
                    id: id.clone(),
                    name: "shell".into(),
                    input: serde_json::json!({}),
                },
            });
            sink.record(&Event::ToolFinished {
                step: 1,
                tool: "shell".into(),
                result: ToolResult {
                    call_id: id,
                    output: "x".repeat(1_000_000),
                    is_error: false,
                    images: vec![],
                },
                duration_ms: 1,
            });
        }
        let session = load(&path);
        let held: usize = session
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Tool(tool) => tool.result.as_ref().map(|result| result.output.len()),
                _ => None,
            })
            .sum();
        assert!(held < 16 * 6100);
        assert!(load_cancelled(&path, &std::sync::atomic::AtomicBool::new(true)).is_err());
        assert!(
            load_cancelled(
                &project.path().join("missing"),
                &std::sync::atomic::AtomicBool::new(false)
            )
            .is_err()
        );
    }

    #[test]
    fn sessions_are_listed_newest_first_and_can_be_replayed() {
        let project = tempfile::tempdir().unwrap();

        write_log(project.path(), "1000.jsonl", "older task");
        write_log(project.path(), "2000-123-1.jsonl", "newer task");

        JsonlSink::create(&project.path().join(TRAJECTORY_DIRECTORY).join("3000.jsonl")).unwrap();

        let sessions = list(project.path());

        let titles: Vec<&str> = sessions.iter().map(|past| past.title.as_str()).collect();

        assert_eq!(titles, ["newer task", "older task"]);

        let session = load(&sessions[0].path);

        assert!(matches!(&session.items[0], Item::User { text, .. } if text == "newer task"));
        assert!(matches!(session.items[1], Item::RunSummary { .. }));
    }

    #[test]
    fn continuing_an_older_chat_moves_it_above_newer_chats() {
        let project = tempfile::tempdir().unwrap();
        write_log(project.path(), "1000.jsonl", "older chat");
        write_log(project.path(), "2000.jsonl", "newer chat");
        let logs = project.path().join(TRAJECTORY_DIRECTORY);
        for (name, updated) in [("1000.jsonl", 3000), ("2000.jsonl", 2000)] {
            File::options()
                .write(true)
                .open(logs.join(name))
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(
                    std::time::UNIX_EPOCH + std::time::Duration::from_millis(updated),
                ))
                .unwrap();
        }
        let sessions = list(project.path());
        assert_eq!(sessions[0].title, "older chat");
        assert_eq!(sessions[0].started_ms, 1000);
        assert_eq!(sessions[0].updated_ms, 3000);
        assert_eq!(sessions[1].title, "newer chat");
    }

    #[test]
    fn a_deleted_session_is_no_longer_listed() {
        let project = tempfile::tempdir().unwrap();

        write_log(project.path(), "1000.jsonl", "keep");
        write_log(project.path(), "2000.jsonl", "drop");

        delete(&list(project.path())[0].path).unwrap();

        let titles: Vec<String> = list(project.path())
            .into_iter()
            .map(|past| past.title)
            .collect();

        assert_eq!(titles, ["keep"]);
    }

    #[test]
    fn listing_again_reads_no_log_a_second_time() {
        let project = tempfile::tempdir().unwrap();

        write_log(project.path(), "1000.jsonl", "first words");

        assert_eq!(list(project.path())[0].title, "first words");

        // The log can no longer be read as one; a list that opened it
        // again would find no instruction in it and leave it out.
        let path = project.path().join(TRAJECTORY_DIRECTORY).join("1000.jsonl");

        fs::write(&path, "not a log any more").unwrap();

        assert_eq!(list(project.path())[0].title, "first words");

        // Deleted, it is forgotten as well as gone.
        delete(&path).unwrap();
        fs::write(&path, "not a log any more").unwrap();

        assert!(list(project.path()).is_empty());
    }

    #[test]
    fn a_project_without_logs_has_no_sessions() {
        let project = tempfile::tempdir().unwrap();

        assert!(list(project.path()).is_empty());
    }
}
