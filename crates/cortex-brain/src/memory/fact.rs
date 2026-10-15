//! Long-term memory: facts about a repository.
//!
//! A fact is a short statement that held across tasks, such as which
//! command runs the tests. Facts are few, shown on every task in the
//! repository, and dropped when the code they mention is gone.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactKind {
    /// A command that checks the project and has passed.
    CheckCommand,

    /// A command that keeps failing here.
    DeadEnd,

    /// A file that several tasks ended up changing.
    HotFile,

    /// Something the agent chose to write down with the `remember` tool.
    Note,

    /// A short description of the project written ahead of any task by
    /// `cortex prepare`. There is one per repository.
    Overview,
}

impl FactKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::CheckCommand => "check_command",
            Self::DeadEnd => "dead_end",
            Self::HotFile => "hot_file",
            Self::Note => "note",
            Self::Overview => "overview",
        }
    }

    /// The order facts are shown in: how to verify first, then where the
    /// work usually is, then notes, then what to avoid.
    pub fn priority(self) -> u8 {
        match self {
            Self::Overview => 0,
            Self::CheckCommand => 1,
            Self::HotFile => 2,
            Self::Note => 3,
            Self::DeadEnd => 4,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fact {
    pub kind: FactKind,

    /// What makes this fact the same fact when it is seen again: the
    /// command, the path, or the note's text.
    pub key: String,

    /// The statement shown to the model.
    pub text: String,

    /// Files the fact is about. If one no longer exists the fact is stale.
    pub paths: Vec<String>,

    /// How many episodes support it.
    pub support: u32,

    pub updated_ms: u64,
}

impl Fact {
    pub fn note(text: &str, paths: Vec<String>, now_ms: u64) -> Self {
        Self {
            kind: FactKind::Note,
            key: text.trim().to_lowercase(),
            text: text.trim().to_string(),
            paths,
            support: 1,
            updated_ms: now_ms,
        }
    }

    /// The project's overview. Writing a new one replaces the old.
    pub fn overview(text: &str, now_ms: u64) -> Self {
        Self {
            kind: FactKind::Overview,
            key: "overview".to_string(),
            text: text.trim().to_string(),
            paths: Vec::new(),
            support: 1,
            updated_ms: now_ms,
        }
    }

    /// Whether every file the fact mentions still exists.
    pub fn still_holds(&self, exists: impl Fn(&str) -> bool) -> bool {
        self.paths.iter().all(|path| exists(path))
    }
}
