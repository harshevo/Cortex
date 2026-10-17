//! Learning from the runs a project has had.
//!
//! Every run that changed something is kept as an episode
//! ([`crate::notes::Episode`]): what was asked, which tools were used,
//! how many steps and tokens it took, which notes it was shown, and how
//! it ended. From those this module works out, without any model:
//!
//! - **Playbooks.** Per kind of task (a fix, a feature, a refactor, …),
//!   how often runs pass, how long they take, and which tools the runs
//!   that passed used and the runs that failed did not. A new task of
//!   that kind is told so before it starts, and given an estimate of its
//!   size for its budget.
//! - **Which notes help.** How runs went when a note was shown. A note
//!   that is shown often and never with a passing run is flagged, so it
//!   can be checked or dropped.
//! - **Candidate skills.** The same sequence of tools in several passing
//!   runs of one kind is a workflow worth writing down. It is offered as
//!   a skill; nothing becomes a skill until the person accepts it, and an
//!   accepted skill is an ordinary file that can be edited or deleted.
//!
//! Nothing learned changes the model. Whether advice helps at all is
//! measured separately, with hold-outs ([`crate::effect`]).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::agent::Outcome;
use crate::briefing::{Briefing, ContextSource};
use crate::notes::{Episode, Notes};
use crate::workspace::Workspace;

/// Fewer similar runs than this say nothing.
const MIN_SAMPLES: usize = 3;

/// The file where dismissed candidates are kept.
pub const LEARNING_FILE: &str = ".cortex/learning.json";

/// What a task is, judged from the words of its instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    Fix,
    Feature,
    Refactor,
    Test,
    Docs,
    Build,
    Other,
}

impl TaskKind {
    pub const ALL: [TaskKind; 7] = [
        TaskKind::Fix,
        TaskKind::Feature,
        TaskKind::Refactor,
        TaskKind::Test,
        TaskKind::Docs,
        TaskKind::Build,
        TaskKind::Other,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TaskKind::Fix => "Fixes",
            TaskKind::Feature => "Features",
            TaskKind::Refactor => "Refactors",
            TaskKind::Test => "Tests",
            TaskKind::Docs => "Docs",
            TaskKind::Build => "Build and setup",
            TaskKind::Other => "Other",
        }
    }

    fn slug(self) -> &'static str {
        match self {
            TaskKind::Fix => "fix",
            TaskKind::Feature => "feature",
            TaskKind::Refactor => "refactor",
            TaskKind::Test => "test",
            TaskKind::Docs => "docs",
            TaskKind::Build => "build",
            TaskKind::Other => "task",
        }
    }
}

/// The kind of task `instruction` asks for.
pub fn kind_of(instruction: &str) -> TaskKind {
    let text = instruction.to_lowercase();
    let words: BTreeSet<&str> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();

    let has = |list: &[&str]| list.iter().any(|word| words.contains(word));

    if has(&[
        "fix", "bug", "broken", "crash", "error", "fails", "failing", "issue", "wrong", "panic",
    ]) {
        TaskKind::Fix
    } else if has(&[
        "refactor", "rename", "extract", "clean", "cleanup", "simplify", "move", "split",
    ]) {
        TaskKind::Refactor
    } else if has(&["test", "tests", "coverage", "spec"]) {
        TaskKind::Test
    } else if has(&[
        "readme",
        "docs",
        "document",
        "documentation",
        "comment",
        "comments",
        "changelog",
    ]) {
        TaskKind::Docs
    } else if has(&[
        "build",
        "ci",
        "dependency",
        "dependencies",
        "upgrade",
        "install",
        "config",
        "setup",
        "deploy",
    ]) {
        TaskKind::Build
    } else if has(&[
        "add",
        "implement",
        "create",
        "support",
        "new",
        "feature",
        "make",
        "allow",
    ]) {
        TaskKind::Feature
    } else {
        TaskKind::Other
    }
}

/// Whether a run did what it was asked, as far as can be told.
pub fn passed(episode: &Episode) -> bool {
    episode.outcome == Outcome::Completed && episode.verified != Some(false)
}

/// What runs of one kind have been like.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct KindStats {
    pub kind: TaskKind,
    pub runs: usize,
    pub passed: usize,
    pub median_steps: u32,
    pub median_tokens: u64,

    /// Tools that most passing runs used and fewer failing runs did.
    pub helpful_tools: Vec<String>,
}

impl KindStats {
    pub fn pass_rate(&self) -> f64 {
        self.passed as f64 / self.runs.max(1) as f64
    }
}

/// How runs went when a note was shown.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NoteValue {
    pub id: u32,
    pub shown: usize,
    pub passed: usize,
}

impl NoteValue {
    /// Shown often and never in a run that passed.
    pub fn doubtful(&self) -> bool {
        self.shown >= MIN_SAMPLES && self.passed == 0
    }
}

/// A workflow seen in several passing runs, offered as a skill.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Candidate {
    pub name: String,
    pub kind: TaskKind,

    /// The tools, in order.
    pub steps: Vec<String>,

    /// How many passing runs followed it.
    pub runs: usize,

    /// A few of their instructions.
    pub examples: Vec<String>,
}

/// Everything learned from a project's runs.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Learned {
    pub runs: usize,
    pub kinds: Vec<KindStats>,
    pub notes: Vec<NoteValue>,
    pub candidates: Vec<Candidate>,
}

/// Learns from `episodes`. Candidates named in `dismissed` are left out.
pub fn learn(episodes: &[Episode], dismissed: &BTreeSet<String>) -> Learned {
    let mut by_kind: BTreeMap<TaskKind, Vec<&Episode>> = BTreeMap::new();

    for episode in episodes {
        by_kind
            .entry(kind_of(&episode.instruction))
            .or_default()
            .push(episode);
    }

    let kinds = by_kind
        .iter()
        .map(|(kind, runs)| stats(*kind, runs))
        .collect();

    let candidates = by_kind
        .iter()
        .filter_map(|(kind, runs)| candidate(*kind, runs))
        .filter(|candidate| !dismissed.contains(&candidate.name))
        .collect();

    Learned {
        runs: episodes.len(),
        kinds,
        notes: note_values(episodes),
        candidates,
    }
}

fn stats(kind: TaskKind, runs: &[&Episode]) -> KindStats {
    let (good, bad): (Vec<&Episode>, Vec<&Episode>) = runs.iter().partition(|run| passed(run));

    let share = |group: &[&Episode], tool: &str| {
        group
            .iter()
            .filter(|run| run.tools.iter().any(|used| used == tool))
            .count() as f64
            / group.len().max(1) as f64
    };

    let tools: BTreeSet<&String> = good.iter().flat_map(|run| &run.tools).collect();

    let mut helpful: Vec<(f64, String)> = tools
        .into_iter()
        .map(|tool| (share(&good, tool), share(&bad, tool), tool))
        .filter(|(with, without, _)| *with >= 0.6 && (bad.is_empty() || with - without >= 0.3))
        .map(|(with, _, tool)| (with, tool.clone()))
        .collect();

    helpful.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));

    KindStats {
        kind,
        runs: runs.len(),
        passed: good.len(),
        median_steps: median(runs.iter().map(|run| u64::from(run.steps))) as u32,
        median_tokens: median(runs.iter().map(|run| run.tokens)),
        helpful_tools: helpful.into_iter().take(5).map(|(_, tool)| tool).collect(),
    }
}

fn median(values: impl Iterator<Item = u64>) -> u64 {
    let mut values: Vec<u64> = values.collect();

    if values.is_empty() {
        return 0;
    }

    values.sort_unstable();
    values[values.len() / 2]
}

fn note_values(episodes: &[Episode]) -> Vec<NoteValue> {
    let mut values: BTreeMap<u32, (usize, usize)> = BTreeMap::new();

    for episode in episodes {
        for id in &episode.notes_shown {
            let entry = values.entry(*id).or_default();

            entry.0 += 1;

            if passed(episode) {
                entry.1 += 1;
            }
        }
    }

    values
        .into_iter()
        .map(|(id, (shown, passed))| NoteValue { id, shown, passed })
        .collect()
}

/// The most common workflow of the passing runs of one kind, when
/// enough of them share it.
fn candidate(kind: TaskKind, runs: &[&Episode]) -> Option<Candidate> {
    let mut counts: HashMap<Vec<String>, Vec<&Episode>> = HashMap::new();

    for run in runs.iter().filter(|run| passed(run)) {
        // Only the tools that do something distinctive: a workflow of
        // reading files alone is no workflow.
        let steps: Vec<String> = run.tools.iter().take(6).cloned().collect();

        if steps.len() >= 3 {
            counts.entry(steps).or_default().push(run);
        }
    }

    let (steps, followed) = counts
        .into_iter()
        .filter(|(_, followed)| followed.len() >= MIN_SAMPLES)
        .max_by(|a, b| a.1.len().cmp(&b.1.len()).then(b.0.cmp(&a.0)))?;

    Some(Candidate {
        name: format!("{}-workflow", kind.slug()),
        kind,
        runs: followed.len(),
        examples: followed
            .iter()
            .rev()
            .take(3)
            .map(|run| run.instruction.clone())
            .collect(),
        steps,
    })
}

impl Candidate {
    /// The skill file this candidate would be.
    pub fn skill(&self) -> String {
        let steps = self
            .steps
            .iter()
            .enumerate()
            .map(|(index, tool)| format!("{}. {}", index + 1, step_for(tool)))
            .collect::<Vec<_>>()
            .join("\n");

        let examples = self
            .examples
            .iter()
            .map(|example| format!("- {example}"))
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            "---\nname: {}\ndescription: The way {} have gone well in this project, learned from {} runs that passed.\nargument-hint: \"[task]\"\n---\n\n\
             Task: $ARGUMENTS\n\n{steps}\n\nLearned from runs such as:\n\n{examples}\n",
            self.name,
            self.kind.label().to_lowercase(),
            self.runs
        )
    }
}

fn step_for(tool: &str) -> String {
    match tool {
        "read_file" | "outline" | "read_symbol" => {
            "Read the code involved before changing it.".into()
        }
        "search" | "semantic_search" | "list_files" => "Find where the behaviour lives.".into(),
        "find_references" | "file_dependencies" | "impact" => {
            "Check what depends on the code you will change.".into()
        }
        "affected_tests" => "Find the tests that cover it.".into(),
        "edit_file" | "write_file" => "Make the change, in small edits.".into(),
        "shell" => "Build and run the tests.".into(),
        "update_plan" => "Write a short plan and keep it current.".into(),
        "save_note" => "Record what you learned for later sessions.".into(),
        "git" => "Look at the history of the code you are changing.".into(),
        other => format!("Use `{other}`."),
    }
}

/// What the next task should expect.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Estimate {
    pub kind: TaskKind,
    pub samples: usize,
    pub steps: u32,
    pub tokens: u64,
    pub pass_rate: f64,
    pub helpful_tools: Vec<String>,
}

/// An estimate for `instruction` from runs of its kind, when there are
/// enough of them.
pub fn estimate(episodes: &[Episode], instruction: &str) -> Option<Estimate> {
    let kind = kind_of(instruction);

    let runs: Vec<&Episode> = episodes
        .iter()
        .filter(|episode| kind_of(&episode.instruction) == kind)
        .collect();

    if runs.len() < MIN_SAMPLES {
        return None;
    }

    let stats = stats(kind, &runs);

    Some(Estimate {
        kind,
        samples: runs.len(),
        steps: stats.median_steps,
        tokens: stats.median_tokens,
        pass_rate: stats.pass_rate(),
        helpful_tools: stats.helpful_tools,
    })
}

impl Estimate {
    /// What the model is told before the task.
    pub fn briefing(&self) -> String {
        let mut text = format!(
            "## What similar tasks took\n\n{} earlier {} in this project: {:.0}% finished with \
             their checks passing, in about {} steps.",
            self.samples,
            self.kind.label().to_lowercase(),
            self.pass_rate * 100.0,
            self.steps
        );

        if !self.helpful_tools.is_empty() {
            let tools = self
                .helpful_tools
                .iter()
                .map(|tool| format!("`{tool}`"))
                .collect::<Vec<_>>()
                .join(", ");

            text.push_str(&format!(
                " The runs that passed used {tools}; most that failed did not."
            ));
        }

        text
    }
}

/// Puts the playbook for the task's kind before it.
pub struct PlaybookSource;

#[async_trait]
impl ContextSource for PlaybookSource {
    async fn brief(&self, instruction: &str, workspace: &Workspace) -> Option<Briefing> {
        let notes = Notes::for_project(workspace.root());
        let instruction = instruction.to_string();

        let estimate =
            tokio::task::spawn_blocking(move || estimate(&notes.episodes(), &instruction))
                .await
                .ok()??;

        Some(Briefing {
            source: "playbook".to_string(),
            text: estimate.briefing(),
            detail: json!({ "estimate": estimate }),
        })
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct LearningFile {
    #[serde(default)]
    dismissed: BTreeSet<String>,
}

/// What a project has learned, read from its files.
pub fn for_project(root: &Path) -> Learned {
    let episodes = Notes::for_project(root).episodes();

    learn(&episodes, &read_file(root).dismissed)
}

fn read_file(root: &Path) -> LearningFile {
    std::fs::read_to_string(root.join(LEARNING_FILE))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Makes a candidate a skill of the project. Returns its file.
pub fn accept(root: &Path, candidate: &Candidate) -> std::io::Result<PathBuf> {
    let folder = root.join(".cortex/skills").join(&candidate.name);

    std::fs::create_dir_all(&folder)?;

    let path = folder.join("SKILL.md");

    if path.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("{} exists already", path.display()),
        ));
    }

    std::fs::write(&path, candidate.skill())?;

    Ok(path)
}

/// Stops offering a candidate.
pub fn dismiss(root: &Path, name: &str) -> std::io::Result<()> {
    let mut file = read_file(root);

    file.dismissed.insert(name.to_string());

    let path = root.join(LEARNING_FILE);

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    std::fs::write(path, serde_json::to_string_pretty(&file)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn episode(instruction: &str, tools: &[&str], passed: bool, steps: u32) -> Episode {
        Episode {
            time: 0,
            instruction: instruction.to_string(),
            outcome: if passed {
                Outcome::Completed
            } else {
                Outcome::StepLimit
            },
            verified: Some(passed),
            steps,
            changed: vec!["a.rs".to_string()],
            tools: tools.iter().map(|tool| tool.to_string()).collect(),
            tokens: u64::from(steps) * 1000,
            notes_shown: vec![1],
        }
    }

    #[test]
    fn kinds_are_told_from_the_words() {
        assert_eq!(
            kind_of("Fix the crash when the list is empty"),
            TaskKind::Fix
        );
        assert_eq!(kind_of("Rename Order to Purchase"), TaskKind::Refactor);
        assert_eq!(kind_of("Add a dark mode"), TaskKind::Feature);
        assert_eq!(kind_of("Update the README"), TaskKind::Docs);
    }

    #[test]
    fn the_tools_of_passing_runs_are_learned() {
        let flow = ["read_file", "edit_file", "affected_tests", "shell"];

        let episodes = vec![
            episode("fix bug one", &flow, true, 10),
            episode("fix bug two", &flow, true, 12),
            episode("fix bug three", &flow, true, 14),
            episode("fix bug four", &["read_file", "edit_file"], false, 40),
        ];

        let learned = learn(&episodes, &BTreeSet::new());
        let fixes = &learned.kinds[0];

        assert_eq!(fixes.runs, 4);
        assert_eq!(fixes.passed, 3);
        assert!(fixes.helpful_tools.contains(&"affected_tests".to_string()));
        assert!(!fixes.helpful_tools.contains(&"read_file".to_string()));

        let candidate = &learned.candidates[0];
        assert_eq!(candidate.name, "fix-workflow");
        assert_eq!(candidate.runs, 3);
        assert!(candidate.skill().contains("name: fix-workflow"));

        let estimate = estimate(&episodes, "fix the login bug").unwrap();
        assert_eq!(estimate.samples, 4);
        assert!(estimate.briefing().contains("75%"));
    }

    #[test]
    fn a_note_never_shown_with_a_pass_is_doubtful() {
        let episodes: Vec<Episode> = (0..3)
            .map(|index| episode(&format!("fix {index}"), &["shell"], false, 5))
            .collect();

        let learned = learn(&episodes, &BTreeSet::new());

        assert!(learned.notes[0].doubtful());
    }

    #[test]
    fn accepted_and_dismissed_candidates_are_kept_on_disk() {
        let project = tempfile::tempdir().unwrap();

        let candidate = Candidate {
            name: "fix-workflow".to_string(),
            kind: TaskKind::Fix,
            steps: vec!["read_file".into(), "edit_file".into(), "shell".into()],
            runs: 3,
            examples: vec!["fix it".into()],
        };

        let path = accept(project.path(), &candidate).unwrap();
        assert!(path.ends_with(".cortex/skills/fix-workflow/SKILL.md"));
        assert!(accept(project.path(), &candidate).is_err());

        dismiss(project.path(), "fix-workflow").unwrap();
        assert!(read_file(project.path()).dismissed.contains("fix-workflow"));
    }
}
