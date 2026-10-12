//! Episodic memory: one record per finished task.

use cortex_core::Outcome;
use serde::{Deserialize, Serialize};

use crate::working::{WorkingMemory, first_line};

/// Limits that keep one episode small enough to show several at once.
const MAX_FILES_READ: usize = 10;
const MAX_COMMANDS: usize = 5;
const MAX_COMMAND_CHARS: usize = 140;
const MAX_REPORT_CHARS: usize = 500;
const MAX_INSTRUCTION_CHARS: usize = 220;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Episode {
    /// Row id in the store. Zero until the episode is saved.
    #[serde(default)]
    pub id: i64,

    pub created_ms: u64,
    pub instruction: String,

    /// How the run ended.
    pub outcome: Option<Outcome>,

    /// Whether the project's checks passed on the result. `None` when no
    /// verification ran, so the result is only the model's claim.
    pub verified: Option<bool>,

    /// Times the verification gate sent the work back before it passed.
    pub rejections: u32,

    pub steps: u32,
    pub tokens: u64,
    pub duration_ms: u64,

    /// The files that were changed: where the answer turned out to be.
    pub files_changed: Vec<String>,

    /// The files opened on the way, in order: the path that led there.
    pub files_read: Vec<String>,

    /// Commands that exited with 0.
    pub commands_worked: Vec<String>,
    #[serde(default)]
    pub calculations: Vec<String>,

    /// Commands that exited with an error, and tool calls that failed.
    pub dead_ends: Vec<String>,

    /// Every check the verification gate ran at the end of the task.
    #[serde(default)]
    pub checks_run: Vec<String>,

    /// The ones among them that failed.
    pub checks_failed: Vec<String>,

    /// The model's own closing report, shortened.
    pub report: String,
}

impl Episode {
    /// Consolidation, step one: compress a finished run into an episode.
    pub fn from_working(memory: &WorkingMemory, created_ms: u64) -> Self {
        let worked = commands(memory, true);

        // A command that failed and later worked unchanged was not a dead
        // end, only a step on the way.
        let mut dead_ends: Vec<String> = commands(memory, false)
            .into_iter()
            .filter(|command| !worked.contains(command))
            .collect();

        dead_ends.extend(
            memory
                .failures
                .iter()
                .map(|failure| clip(failure, MAX_COMMAND_CHARS)),
        );
        dead_ends.truncate(MAX_COMMANDS);

        let checks_run = memory
            .checks
            .iter()
            .map(|check| check.command.clone())
            .collect();

        let checks_failed = memory
            .checks
            .iter()
            .filter(|check| !check.passed)
            .map(|check| check.command.clone())
            .collect();

        Self {
            id: 0,
            created_ms,
            instruction: memory.instruction.clone(),
            outcome: memory.outcome,
            verified: memory.verified,
            rejections: memory.rejections,
            steps: memory.steps,
            tokens: memory.tokens,
            duration_ms: memory.duration_ms,
            files_changed: memory.files_changed.clone(),
            files_read: memory
                .files_read
                .iter()
                .filter(|path| !memory.files_changed.contains(path))
                .take(MAX_FILES_READ)
                .cloned()
                .collect(),
            commands_worked: worked.into_iter().take(MAX_COMMANDS).collect(),
            calculations: memory.calculations.iter().rev().take(5).cloned().collect(),
            dead_ends,
            checks_run,
            checks_failed,
            report: clip(&memory.last_text, MAX_REPORT_CHARS),
        }
    }

    /// An episode with nothing in it teaches nothing and is not stored.
    pub fn is_empty(&self) -> bool {
        self.files_changed.is_empty()
            && self.files_read.is_empty()
            && self.commands_worked.is_empty()
            && self.calculations.is_empty()
            && self.report.trim().is_empty()
    }

    /// The checks that passed at the end of the task.
    pub fn checks_passed(&self) -> Vec<&str> {
        self.checks_run
            .iter()
            .filter(|command| !self.checks_failed.contains(command))
            .map(String::as_str)
            .collect()
    }

    /// The text that stands for this episode when it is compared with a
    /// new task.
    pub fn search_text(&self) -> String {
        format!(
            "{}\n{}\n{}",
            self.instruction,
            self.files_changed.join(" "),
            self.report
        )
    }

    /// One line on how the task went.
    pub fn result_line(&self) -> String {
        let verdict = match (self.verified, self.outcome) {
            (Some(true), _) => "the project's checks passed",
            (Some(false), _) => "the project's checks still FAILED at the end",
            (None, Some(Outcome::Completed)) => "finished, not verified by checks",
            (None, _) => "did not finish",
        };

        let rejected = match self.rejections {
            0 => String::new(),
            count => format!(", after the checks rejected {count} earlier attempt(s)"),
        };

        format!("{verdict}{rejected}; {} steps", self.steps)
    }

    /// The episode as shown to the model. `exists` says whether a path is
    /// still in the project, so files deleted since are left out.
    pub fn render(&self, exists: impl Fn(&str) -> bool) -> String {
        let mut text = format!("\"{}\"\n", clip(&self.instruction, MAX_INSTRUCTION_CHARS));
        if !self.calculations.is_empty() {
            text.push_str(&format!(
                "Calculated (double precision): {}\n",
                self.calculations.join("; ")
            ));
        }

        text.push_str(&format!("   Result: {}.\n", self.result_line()));

        let present = |paths: &[String]| -> Vec<String> {
            paths.iter().filter(|path| exists(path)).cloned().collect()
        };

        let mut line = |label: &str, items: Vec<String>| {
            if !items.is_empty() {
                text.push_str(&format!("   {label}: {}\n", items.join(", ")));
            }
        };

        line("Changed", present(&self.files_changed));
        line("Read on the way", present(&self.files_read));
        line("Commands that worked", quoted(&self.commands_worked));
        line("Dead ends", quoted(&self.dead_ends));

        if !self.report.is_empty() {
            text.push_str(&format!(
                "   Its report: {}\n",
                self.report.replace('\n', " ")
            ));
        }

        text
    }
}

/// The run's shell commands that did or did not succeed, normalised and
/// without repeats.
fn commands(memory: &WorkingMemory, succeeded: bool) -> Vec<String> {
    let mut commands = Vec::new();

    for record in &memory.commands {
        if record.succeeded != succeeded {
            continue;
        }

        let Some(command) = normalize_command(&record.command) else {
            continue;
        };

        if !commands.contains(&command) {
            commands.push(command);
        }
    }

    commands
}

/// Reduces a command to the part worth remembering: `npm test 2>&1 | tail
/// -30` is the same knowledge as `npm test`. Multi-line scripts are one-off
/// and are not kept.
pub fn normalize_command(command: &str) -> Option<String> {
    let command = command.trim();

    if command.lines().count() != 1 {
        return None;
    }

    let mut core = command;

    for cut in [" | ", " 2>&1", " > ", " >> ", "; echo"] {
        if let Some(position) = core.find(cut) {
            core = &core[..position];
        }
    }

    let core = core.split_whitespace().collect::<Vec<_>>().join(" ");

    if core.is_empty() || core.chars().count() > MAX_COMMAND_CHARS {
        return None;
    }

    Some(core)
}

fn quoted(items: &[String]) -> Vec<String> {
    items
        .iter()
        .map(|item| format!("`{}`", first_line(item)))
        .collect()
}

pub fn clip(text: &str, max_chars: usize) -> String {
    let text = text.trim();

    if text.chars().count() <= max_chars {
        return text.to_string();
    }

    let clipped: String = text.chars().take(max_chars).collect();

    format!("{clipped}...")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::working::CommandRecord;

    fn command(command: &str, succeeded: bool) -> CommandRecord {
        CommandRecord {
            command: command.to_string(),
            succeeded,
        }
    }

    #[test]
    fn commands_are_normalised() {
        assert_eq!(
            normalize_command("npm  test 2>&1 | tail -30").as_deref(),
            Some("npm test")
        );

        assert_eq!(normalize_command("cat > x <<'EOF'\nhello\nEOF"), None);
    }

    #[test]
    fn an_episode_keeps_what_worked_and_what_did_not() {
        let memory = WorkingMemory {
            instruction: "fix the calendar".to_string(),
            files_read: vec!["a.ts".to_string(), "b.ts".to_string()],
            files_changed: vec!["b.ts".to_string()],
            commands: vec![
                command("npx jest", false),
                command("npm test | tail -5", false),
                command("npm test", true),
            ],
            last_text: "Fixed the week grid.".to_string(),
            verified: Some(true),
            steps: 7,
            ..WorkingMemory::default()
        };

        let episode = Episode::from_working(&memory, 1);

        assert_eq!(episode.files_changed, ["b.ts"]);
        assert_eq!(episode.files_read, ["a.ts"]);
        assert_eq!(episode.commands_worked, ["npm test"]);

        // `npm test` failed first and then passed, so only jest is a dead end.
        assert_eq!(episode.dead_ends, ["npx jest"]);

        let rendered = episode.render(|path| path != "a.ts");

        assert!(rendered.contains("the project's checks passed; 7 steps"));
        assert!(rendered.contains("Changed: b.ts"));
        assert!(!rendered.contains("a.ts"));
        assert!(rendered.contains("Dead ends: `npx jest`"));
    }
}
