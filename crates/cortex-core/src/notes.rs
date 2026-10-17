//! What the project has taught, kept between sessions.
//!
//! Two records live in the project's `.cortex` folder:
//!
//! - **Notes** (`notes.json`): facts worth knowing next time, each with
//!   its kind (a convention, a piece of architecture, a decision, a
//!   procedure, a pitfall), where it came from, when, and the files it is
//!   about. The model writes them with `save_note` and retires them with
//!   `drop_note`; the person can edit the file.
//! - **Episodes** (`episodes.jsonl`): one line per finished run: what was
//!   asked, how it ended, whether it was verified, and which files it
//!   changed.
//!
//! Before each task the [`NotesSource`] puts the smallest useful set in
//! front of it: every convention and architecture note, the other notes
//! that share words with the task, and the few earlier runs most like
//! it. A note about files that have changed since it was written is shown
//! marked as possibly out of date, so it is checked rather than trusted.
//!
//! Nothing is learned behind the person's back: notes are plain text the
//! model chose to write, and nothing is kept outside the project.

use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::agent::Outcome;
use crate::briefing::{Briefing, ContextSource};
use crate::trajectory::{Event, TrajectorySink, unix_millis};
use crate::workspace::Workspace;

pub const NOTES_FILE: &str = ".cortex/notes.json";
pub const EPISODES_FILE: &str = ".cortex/episodes.jsonl";

/// Notes shown before a task at most, and characters of them.
const MAX_NOTES: usize = 15;
const MAX_CHARS: usize = 3_000;

/// Earlier runs shown before a task at most.
const MAX_EPISODES: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// How things are done here: style, commands, layout.
    #[default]
    Convention,

    /// How the parts fit together.
    Architecture,

    /// A choice that was made, and why.
    Decision,

    /// A way of doing something that worked.
    Procedure,

    /// Something that went wrong, and how to avoid it.
    Pitfall,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Convention => "convention",
            Self::Architecture => "architecture",
            Self::Decision => "decision",
            Self::Procedure => "procedure",
            Self::Pitfall => "pitfall",
        }
    }

    /// Shown before every task, whatever it is about.
    fn always_shown(self) -> bool {
        matches!(self, Self::Convention | Self::Architecture)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub id: u32,

    #[serde(default)]
    pub kind: Kind,

    pub text: String,

    /// The files the note is about, relative to the project, with a
    /// fingerprint of each as it was when the note was written.
    #[serde(default)]
    pub files: BTreeMap<String, String>,

    /// Milliseconds since 1970.
    pub created: u128,

    /// Who wrote it: `model` or `person`.
    #[serde(default = "model")]
    pub source: String,

    /// What it was learned during, in a line.
    #[serde(default)]
    pub learned_during: String,
}

fn model() -> String {
    "model".to_string()
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct NotesFile {
    #[serde(default)]
    notes: Vec<Note>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Episode {
    pub time: u128,
    pub instruction: String,
    pub outcome: Outcome,
    pub verified: Option<bool>,
    pub steps: u32,

    #[serde(default)]
    pub changed: Vec<String>,

    /// The tools the run used, each once, in the order first used.
    #[serde(default)]
    pub tools: Vec<String>,

    /// Tokens in and out.
    #[serde(default)]
    pub tokens: u64,

    /// The notes the run was shown, to learn which ones help.
    #[serde(default)]
    pub notes_shown: Vec<u32>,
}

/// The notes of one project.
#[derive(Debug, Clone)]
pub struct Notes {
    root: PathBuf,
}

impl Notes {
    pub fn for_project(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    pub fn all(&self) -> Vec<Note> {
        std::fs::read_to_string(self.root.join(NOTES_FILE))
            .ok()
            .and_then(|text| serde_json::from_str::<NotesFile>(&text).ok())
            .map(|file| file.notes)
            .unwrap_or_default()
    }

    /// Saves a note and returns its id. `replaces` retires an earlier
    /// note that this one corrects.
    pub fn save(
        &self,
        kind: Kind,
        text: &str,
        files: &[String],
        learned_during: &str,
        replaces: Option<u32>,
    ) -> std::io::Result<u32> {
        let mut notes = self.all();

        if let Some(old) = replaces {
            notes.retain(|note| note.id != old);
        }

        let id = notes.iter().map(|note| note.id).max().unwrap_or(0) + 1;

        let files = files
            .iter()
            .map(|file| (file.clone(), fingerprint(&self.root.join(file))))
            .collect();

        notes.push(Note {
            id,
            kind,
            text: text.trim().to_string(),
            files,
            created: unix_millis(),
            source: model(),
            learned_during: one_line(learned_during, 120),
        });

        self.write(notes)?;

        Ok(id)
    }

    /// Retires a note. Returns whether there was one.
    pub fn drop(&self, id: u32) -> std::io::Result<bool> {
        let mut notes = self.all();
        let before = notes.len();

        notes.retain(|note| note.id != id);

        if notes.len() == before {
            return Ok(false);
        }

        self.write(notes)?;

        Ok(true)
    }

    /// The files of `note` that have changed or gone since it was written.
    pub fn stale_files(&self, note: &Note) -> Vec<String> {
        note.files
            .iter()
            .filter(|(file, print)| fingerprint(&self.root.join(file)) != **print)
            .map(|(file, _)| file.clone())
            .collect()
    }

    pub fn episodes(&self) -> Vec<Episode> {
        std::fs::read_to_string(self.root.join(EPISODES_FILE))
            .map(|text| {
                text.lines()
                    .filter_map(|line| serde_json::from_str(line).ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn record_episode(&self, episode: &Episode) -> std::io::Result<()> {
        let path = self.root.join(EPISODES_FILE);

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;

        writeln!(file, "{}", serde_json::to_string(episode)?)
    }

    fn write(&self, notes: Vec<Note>) -> std::io::Result<()> {
        let path = self.root.join(NOTES_FILE);

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let text = serde_json::to_string_pretty(&NotesFile { notes })?;

        // Written beside and moved into place, so a crash cannot leave
        // half a file.
        let temporary = path.with_extension("json.tmp");

        std::fs::write(&temporary, text)?;
        std::fs::rename(temporary, path)
    }

    /// The briefing for `instruction`: the notes and earlier runs that
    /// matter to it. `None` when there is nothing.
    pub fn briefing(&self, instruction: &str) -> Option<(String, Vec<u32>)> {
        let words = words(instruction);

        let notes = self.all();

        let mut chosen: Vec<&Note> = notes
            .iter()
            .filter(|note| note.kind.always_shown())
            .collect();

        let mut related: Vec<(usize, &Note)> = notes
            .iter()
            .filter(|note| !note.kind.always_shown())
            .map(|note| (overlap(&words, &note_words(note)), note))
            .filter(|(shared, _)| *shared > 0)
            .collect();

        related.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.created.cmp(&a.1.created)));

        chosen.extend(related.into_iter().map(|(_, note)| note));
        chosen.truncate(MAX_NOTES);

        let mut text = String::new();
        let mut shown = Vec::new();

        for note in chosen {
            let stale = self.stale_files(note);

            let mut line = format!("- [{}] {}", note.kind.label(), one_line(&note.text, 400));

            if !stale.is_empty() {
                line.push_str(&format!(
                    " (may be out of date: {} changed since)",
                    stale.join(", ")
                ));
            }

            line.push_str(&format!(" (note {})\n", note.id));

            if text.len() + line.len() > MAX_CHARS {
                break;
            }

            text.push_str(&line);
            shown.push(note.id);
        }

        let mut episodes: Vec<(usize, Episode)> = self
            .episodes()
            .into_iter()
            .map(|episode| (overlap(&words, &self::words(&episode.instruction)), episode))
            .filter(|(shared, _)| *shared >= 2)
            .collect();

        episodes.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.time.cmp(&a.1.time)));
        episodes.truncate(MAX_EPISODES);

        let mut runs = String::new();

        for (_, episode) in &episodes {
            let ending = match (episode.outcome, episode.verified) {
                (Outcome::Completed, Some(true)) => "finished, checks passed",
                (Outcome::Completed, Some(false)) => "finished, checks failing",
                (Outcome::Completed, None) => "finished, not verified",
                (Outcome::StepLimit, _) => "ran out of steps",
                (Outcome::Truncated, _) => "cut off",
                (Outcome::Refused, _) => "refused",
                (Outcome::Stuck, _) => "stopped, stuck",
                (Outcome::OverBudget, _) => "over budget",
            };

            let changed = match episode.changed.len() {
                0 => String::new(),
                _ => format!("; changed {}", episode.changed.join(", ")),
            };

            runs.push_str(&format!(
                "- \"{}\": {ending}{changed}\n",
                one_line(&episode.instruction, 140)
            ));
        }

        if text.is_empty() && runs.is_empty() {
            return None;
        }

        let mut briefing = String::from("## Project notes\n\n");

        if !text.is_empty() {
            briefing.push_str(
                "What earlier sessions recorded about this project. Check a note marked out \
                 of date before relying on it, and correct a wrong note with `save_note` \
                 (`replaces`).\n\n",
            );
            briefing.push_str(&text);
        }

        if !runs.is_empty() {
            briefing.push_str("\nSimilar earlier tasks:\n");
            briefing.push_str(&runs);
        }

        Some((briefing, shown))
    }
}

/// Puts the notes before each task.
pub struct NotesSource;

#[async_trait]
impl ContextSource for NotesSource {
    async fn brief(&self, instruction: &str, workspace: &Workspace) -> Option<Briefing> {
        let notes = Notes::for_project(workspace.root());

        let instruction = instruction.to_string();

        let (text, shown) = tokio::task::spawn_blocking(move || notes.briefing(&instruction))
            .await
            .ok()??;

        Some(Briefing {
            source: "notes".to_string(),
            detail: json!({ "notes": shown }),
            text,
        })
    }
}

/// Records each finished run as an episode.
pub struct EpisodeSink {
    notes: Notes,
    current: Mutex<Option<Current>>,
}

/// What is known of the run in progress.
#[derive(Default)]
struct Current {
    instruction: String,
    changed: HashSet<String>,
    tools: Vec<String>,
    notes_shown: Vec<u32>,
}

impl EpisodeSink {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            notes: Notes::for_project(root),
            current: Mutex::new(None),
        }
    }
}

impl TrajectorySink for EpisodeSink {
    fn record(&self, event: &Event) {
        let mut current = self
            .current
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        match event {
            Event::RunStarted { instruction, .. } => {
                *current = Some(Current {
                    instruction: instruction.clone(),
                    ..Current::default()
                });
            }

            Event::ContextPrepared { source, detail, .. } if source == "notes" => {
                if let Some(run) = current.as_mut() {
                    run.notes_shown = detail
                        .get("notes")
                        .and_then(|notes| serde_json::from_value(notes.clone()).ok())
                        .unwrap_or_default();
                }
            }

            Event::ToolStarted { call, .. } => {
                if let Some(run) = current.as_mut()
                    && !run.tools.contains(&call.name)
                {
                    run.tools.push(call.name.clone());
                }

                let changing = matches!(
                    call.name.as_str(),
                    "edit_file" | "write_file" | "delete_file" | "move_file"
                );

                if changing
                    && let Some(Current { changed, .. }) = current.as_mut()
                    && let Some(path) = call
                        .input
                        .get("path")
                        .or_else(|| call.input.get("to"))
                        .and_then(|path| path.as_str())
                {
                    changed.insert(path.to_string());
                }
            }

            Event::RunFinished {
                outcome,
                steps,
                verified,
                usage,
                ..
            } => {
                let Some(Current {
                    instruction,
                    changed,
                    tools,
                    notes_shown,
                }) = current.take()
                else {
                    return;
                };

                // A question answered changes nothing and teaches little.
                if changed.is_empty() {
                    return;
                }

                let mut changed: Vec<String> = changed.into_iter().collect();
                changed.sort();
                changed.truncate(12);

                let episode = Episode {
                    time: unix_millis(),
                    instruction: task_of(&instruction),
                    outcome: *outcome,
                    verified: *verified,
                    steps: *steps,
                    changed,
                    tools,
                    tokens: usage.total(),
                    notes_shown,
                };

                if let Err(error) = self.notes.record_episode(&episode) {
                    eprintln!("warning: the run could not be recorded: {error}");
                }
            }

            _ => {}
        }
    }
}

/// What the model is told about notes.
pub fn note() -> String {
    "Project notes. When you learn something about this project that would save a later \
     session time (how to build or test it, a convention, how parts fit together, a decision \
     and its reason, a trap you fell into and how you got out), record it with `save_note`, \
     naming the files it is about. Keep each note to one fact in a sentence or two. Do not \
     record the progress of the current task. When a note you were shown turns out to be \
     wrong, replace it (`replaces`) or remove it with `drop_note`."
        .to_string()
}

/// The task itself, without the briefings placed before it.
fn task_of(instruction: &str) -> String {
    let task = instruction
        .rsplit_once("## Task\n\n")
        .map(|(_, task)| task)
        .unwrap_or(instruction);

    one_line(task, 200)
}

/// FNV-1a over the file's bytes; `missing` when it cannot be read.
fn fingerprint(path: &Path) -> String {
    let Ok(bytes) = std::fs::read(path) else {
        return "missing".to_string();
    };

    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;

    for byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }

    format!("{hash:016x}")
}

const STOP_WORDS: [&str; 24] = [
    "the", "and", "for", "with", "this", "that", "from", "into", "are", "was", "but", "not", "you",
    "can", "should", "make", "add", "fix", "use", "when", "what", "how", "all", "any",
];

fn words(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '_')
        .map(str::to_lowercase)
        .filter(|word| word.len() > 2 && !STOP_WORDS.contains(&word.as_str()))
        .collect()
}

fn note_words(note: &Note) -> HashSet<String> {
    let mut found = words(&note.text);

    for file in note.files.keys() {
        found.extend(words(file));
    }

    found
}

fn overlap(one: &HashSet<String>, other: &HashSet<String>) -> usize {
    one.intersection(other).count()
}

fn one_line(text: &str, max_chars: usize) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");

    match line.chars().count() > max_chars {
        true => format!("{}…", line.chars().take(max_chars).collect::<String>()),
        false => line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conventions_always_show_and_other_notes_when_related() {
        let project = tempfile::tempdir().unwrap();
        let notes = Notes::for_project(project.path());

        notes
            .save(
                Kind::Convention,
                "Tests run with `cargo nextest`.",
                &[],
                "",
                None,
            )
            .unwrap();
        notes
            .save(
                Kind::Pitfall,
                "The parser panics on empty tokens.",
                &[],
                "",
                None,
            )
            .unwrap();

        let (text, shown) = notes.briefing("Rename the logger").unwrap();

        assert!(text.contains("cargo nextest"));
        assert!(!text.contains("parser"));
        assert_eq!(shown, vec![1]);

        let (text, _) = notes.briefing("Fix the parser crash on tokens").unwrap();

        assert!(text.contains("parser panics"));
    }

    #[test]
    fn a_note_about_a_changed_file_is_marked() {
        let project = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("auth.rs"), "fn login() {}").unwrap();

        let notes = Notes::for_project(project.path());

        notes
            .save(
                Kind::Architecture,
                "Login goes through `auth.rs`.",
                &["auth.rs".to_string()],
                "",
                None,
            )
            .unwrap();

        let (text, _) = notes.briefing("anything").unwrap();
        assert!(!text.contains("may be out of date"));

        std::fs::write(project.path().join("auth.rs"), "fn sign_in() {}").unwrap();

        let (text, _) = notes.briefing("anything").unwrap();
        assert!(text.contains("may be out of date: auth.rs"));
    }

    #[test]
    fn a_replacing_note_retires_the_old_one() {
        let project = tempfile::tempdir().unwrap();
        let notes = Notes::for_project(project.path());

        let old = notes
            .save(Kind::Convention, "Use tabs.", &[], "", None)
            .unwrap();
        notes
            .save(Kind::Convention, "Use four spaces.", &[], "", Some(old))
            .unwrap();

        let all = notes.all();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].text, "Use four spaces.");

        assert!(notes.drop(all[0].id).unwrap());
        assert!(notes.all().is_empty());
    }

    #[test]
    fn a_run_that_changed_files_is_recorded_and_recalled_for_a_similar_task() {
        let project = tempfile::tempdir().unwrap();
        let sink = EpisodeSink::new(project.path());

        sink.record(&Event::RunStarted {
            instruction: "## Notes\n\n## Task\n\nAdd retry to the payment client".to_string(),
            provider: "p".to_string(),
            model: "m".to_string(),
        });
        sink.record(&Event::ToolStarted {
            step: 1,
            call: crate::message::ToolCall {
                id: "1".to_string(),
                name: "edit_file".to_string(),
                input: json!({ "path": "src/payment.rs" }),
            },
        });
        sink.record(&Event::RunFinished {
            outcome: Outcome::Completed,
            steps: 4,
            usage: Default::default(),
            duration_ms: 1,
            verified: Some(true),
        });

        let notes = Notes::for_project(project.path());

        assert_eq!(
            notes.episodes()[0].instruction,
            "Add retry to the payment client"
        );

        let (text, _) = notes.briefing("Payment client retry is too slow").unwrap();
        assert!(text.contains("checks passed; changed src/payment.rs"));
    }

    #[test]
    fn nothing_is_written_until_something_is_saved() {
        let project = tempfile::tempdir().unwrap();
        let notes = Notes::for_project(project.path());

        assert!(notes.briefing("anything").is_none());
        assert!(!project.path().join(".cortex").exists());
    }
}
