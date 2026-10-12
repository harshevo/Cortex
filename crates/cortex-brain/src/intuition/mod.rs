//! Intuition: a second brain beside the decision controller.
//!
//! The controller answers "what kind of step next?". This answers "where,
//! and with what?": which file a task like this one usually ends in, what
//! was done after an error like this one. It is an associative network
//! that grows a neuron for each word, file, command and error it meets
//! and learns synapses between them from runs whose checks passed.
//!
//! It speaks in three places, and says nothing when it has nothing:
//!
//! - **Before the task**, from the task's words alone: the files that
//!   tasks worded like it went on to change.
//! - **Between steps**, from where the run is now: what passing runs went
//!   on to from the file just read or the error just met.
//! - **On a long run**, about unfinished business: a check that failed
//!   and was left, edits that nothing has checked. See [`loops`].
//!
//! The task's words stay fully active for the whole run, however long,
//! while everything else fades step by step. So what the run is *for*
//! weighs on a recall at step 50 as it did at step 1, and a file opened
//! late is still tied to the task it was opened for.
//!
//! What this is and is not: the units are neurons with weighted synapses
//! and the learning rules are the textbook ones, but a neuron here stands
//! for one whole concept. It is a learned index of what led to what, of
//! the kind a person calls a hunch. It finds nothing that no earlier run
//! found.
//!
//! - [`concept`] reads a run's events as concepts.
//! - [`network`] holds the neurons and synapses, learns and recalls.
//! - [`loops`] keeps track of unfinished business.

pub mod concept;
pub mod loops;
pub mod network;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use async_trait::async_trait;
use cortex_core::{
    Advice, Adviser, Briefing, ContextSource, Event, Outcome, ToolCall, TrajectorySink, Workspace,
};
use serde_json::{Value, json};

pub use concept::{Concept, Kind};
pub use network::{Network, Picture, PictureNeuron};

use loops::OpenLoops;
use network::{Evidence, Verdict};

/// Runs the network must have learned from before it speaks.
pub const MIN_RUNS: u32 = 4;

/// Share of its activity a concept keeps from one step to the next. Task
/// words do not fade.
const FADE: f32 = 0.85;

/// Activity below this no longer tags synapses or drives recall.
const FAINT: f32 = 0.05;

/// A recalled concept weaker than this is not worth a line. Used until
/// enough hunches have been judged to set the threshold from the record;
/// see [`crate::calibration`].
const THRESHOLD: f32 = 0.5;

/// The share of hunches that should turn out right once the threshold is
/// set from the record.
const PRECISION: f32 = 0.7;

/// Files foreseen at least this strongly at the start of a run are kept
/// for the record whether or not they are mentioned, so the record also
/// knows what lies below the threshold.
const FORESEEN_FLOOR: f32 = 0.2;

/// Files kept for the record per run.
const FORESEEN: usize = 5;

/// A recalled concept is only mentioned beside a stronger one if it has
/// at least this share of the strongest's activity.
const RIVAL_SHARE: f32 = 0.6;

/// Concepts mentioned at once.
const MENTIONED: usize = 3;

/// How active a concept becomes by the way it was met.
const EDITED: f32 = 1.0;
const WRITTEN: f32 = 0.6;
const READ: f32 = 0.4;
const RAN: f32 = 1.0;
const FAILED: f32 = 1.0;

/// The name advice and briefings are logged under.
pub const SOURCE: &str = "intuition";

#[derive(Default)]
struct Run {
    /// The neurons of the task's words, active throughout.
    words: Vec<u32>,

    /// How active every other concept met in this run still is.
    trace: HashMap<u32, f32>,

    evidence: Evidence,

    /// Files already opened or changed and commands already run: nothing
    /// to be reminded of.
    visited: HashSet<u32>,

    /// Already mentioned in this run.
    mentioned: HashSet<u32>,

    /// The files that came to mind when the task was read, with their
    /// strength, and the files the run then changed: together they say
    /// how far the network's strengths can be trusted.
    foreseen: Vec<(u32, f32)>,
    changed: HashSet<u32>,

    pending: HashMap<String, ToolCall>,

    loops: OpenLoops,
    step: u32,
}

struct State {
    network: Network,
    run: Run,
}

/// One thing brought to mind.
#[derive(Debug, Clone, PartialEq)]
pub struct Hunch {
    pub concept: Concept,

    /// From 0 to 1.
    pub strength: f32,

    /// The active concepts that brought it to mind, strongest first.
    pub because: Vec<String>,
}

pub struct Intuition {
    state: Mutex<State>,

    /// Where the network is kept between runs.
    file: Option<PathBuf>,
}

impl Intuition {
    /// An intuition that starts from the network saved in `file`, if
    /// there is one.
    pub fn new(file: Option<PathBuf>) -> Self {
        let network = match &file {
            Some(file) => Network::load_or_new(file),
            None => Network::default(),
        };

        Self {
            state: Mutex::new(State {
                network,
                run: Run::default(),
            }),
            file,
        }
    }

    /// Runs learned from so far.
    pub fn runs(&self) -> u32 {
        self.state().network.runs()
    }

    pub fn neurons(&self) -> usize {
        self.state().network.neuron_count()
    }

    pub fn synapses(&self) -> usize {
        self.state().network.synapse_count()
    }

    /// What the current state of the run brings to mind, strongest first,
    /// without marking any of it as mentioned. Empty until the network
    /// has learned from [`MIN_RUNS`] runs.
    pub fn hunches(&self) -> Vec<Hunch> {
        self.state().hunches()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn save(&self, state: &State) {
        if let Some(file) = &self.file
            && let Err(error) = state.network.save(file)
        {
            eprintln!("warning: the intuition could not save what it learned: {error}");
        }
    }
}

impl State {
    fn rebase(&mut self, mut latest: Network) {
        let mut mapped = HashMap::new();
        let mut map = |id: u32| {
            *mapped
                .entry(id)
                .or_insert_with(|| latest.neuron(self.network.concept(id).clone()))
        };
        self.run.words = self.run.words.iter().map(|id| map(*id)).collect();
        self.run.trace = self
            .run
            .trace
            .iter()
            .map(|(id, value)| (map(*id), *value))
            .collect();
        self.run.visited = self.run.visited.iter().map(|id| map(*id)).collect();
        self.run.mentioned = self.run.mentioned.iter().map(|id| map(*id)).collect();
        self.run.changed = self.run.changed.iter().map(|id| map(*id)).collect();
        self.run.foreseen = self
            .run
            .foreseen
            .iter()
            .map(|(id, value)| (map(*id), *value))
            .collect();
        self.run.evidence.active = self
            .run
            .evidence
            .active
            .iter()
            .map(|(id, value)| (map(*id), *value))
            .collect();
        self.run.evidence.tags = self
            .run
            .evidence
            .tags
            .iter()
            .map(|((a, b), value)| ((map(*a), map(*b)), *value))
            .collect();
        self.network = latest;
    }

    /// A concept was met: tag the synapses from everything still active
    /// onto it, then make it active.
    fn meet(&mut self, concept: Concept, activity: f32) -> u32 {
        let kind = concept.kind;
        let calculation = kind == Kind::Command && concept.text.starts_with("calculate ");
        let neuron = self.network.neuron(concept);

        let words = self.run.words.iter().map(|word| (*word, 1.0));
        let fading = self.run.trace.iter().map(|(other, trace)| (*other, *trace));

        let before: Vec<(u32, f32)> = words.chain(fading).collect();

        for (other, trace) in before {
            let from = self.network.concept(other).kind;

            // A task's words are tied to the files it changed, not to
            // every file it looked at on the way.
            let passing_look = from == Kind::Word && kind == Kind::File && activity < WRITTEN;

            let leads = other != neuron
                && trace >= FAINT
                && (from.may_lead_to(kind) || (from == Kind::Word && calculation))
                && !passing_look;

            if leads {
                let tag = self.run.evidence.tags.entry((other, neuron)).or_default();

                *tag = tag.max(trace * activity);
            }
        }

        for held in [&mut self.run.trace, &mut self.run.evidence.active] {
            let level = held.entry(neuron).or_default();

            *level = level.max(activity);
        }

        neuron
    }

    fn start(&mut self, instruction: &str) {
        self.run = Run::default();

        for word in concept::task_words(instruction) {
            let neuron = self.network.neuron(Concept::new(Kind::Word, word));

            self.run.words.push(neuron);
            self.run.evidence.active.insert(neuron, 1.0);
        }

        let words: Vec<(u32, f32)> = self.run.words.iter().map(|word| (*word, 1.0)).collect();

        self.run.foreseen = self
            .network
            .recall(&words)
            .into_iter()
            .filter(|(neuron, strength)| {
                *strength >= FORESEEN_FLOOR && self.network.concept(*neuron).kind == Kind::File
            })
            .take(FORESEEN)
            .collect();
    }

    fn next_step(&mut self, step: u32) {
        self.run.step = step;

        for trace in self.run.trace.values_mut() {
            *trace *= FADE;
        }

        self.run.trace.retain(|_, trace| *trace >= FAINT);
    }

    fn tool_finished(&mut self, call: &ToolCall, output: &str, is_error: bool) {
        let step = self.run.step;
        if call.name == "calculate" {
            let expression = call.input["expression"].as_str().unwrap_or_default();
            let text: String = format!("calculate {expression}").chars().take(80).collect();
            if is_error {
                self.meet(
                    Concept::new(Kind::Error, output.chars().take(80).collect::<String>()),
                    FAILED,
                );
            } else {
                let neuron = self.meet(Concept::new(Kind::Command, text), RAN);
                self.run.visited.insert(neuron);
            }
            return;
        }

        if call.name == "shell" {
            let line = call
                .input
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or_default();

            let commands = concept::commands_of(line);
            let passed = !is_error && output.starts_with("exit code: 0");

            self.ran(&commands, passed, output, step);

            return;
        }

        let Some(path) = concept::file_of(&call.input) else {
            return;
        };

        let activity = match call.name.as_str() {
            "edit_file" => EDITED,
            "write_file" => WRITTEN,
            _ => READ,
        };

        if is_error {
            // A file that is not there any more should stop coming to mind.
            let gone = ["No such file", "not found", "does not exist"]
                .iter()
                .any(|sign| output.contains(sign));

            if gone && let Some(neuron) = self.network.find(&Concept::new(Kind::File, path)) {
                self.network.silence(neuron);
            }

            return;
        }

        if activity > READ {
            self.run.loops.edited(&path, step);
        }

        let neuron = self.meet(Concept::new(Kind::File, path), activity);

        self.run.visited.insert(neuron);

        if activity > READ {
            self.run.changed.insert(neuron);
        }
    }

    fn ran(&mut self, commands: &[String], passed: bool, output: &str, step: u32) {
        self.run.loops.ran(commands, passed, step);

        if passed {
            for command in commands {
                let neuron = self.meet(Concept::new(Kind::Command, command.clone()), RAN);

                self.run.visited.insert(neuron);
            }
        } else {
            // A command that has just failed is not one to suggest.
            for command in commands {
                let known = self
                    .network
                    .find(&Concept::new(Kind::Command, command.clone()));

                self.run.visited.extend(known);
            }

            if let Some(error) = concept::error_of(output) {
                self.meet(Concept::new(Kind::Error, error), FAILED);
            }
        }
    }

    fn finish(&mut self, outcome: Outcome, verified: Option<bool>) {
        let verdict = match (verified, outcome) {
            (Some(true), _) => Verdict::Passed,
            (Some(false), _) => Verdict::Failed,
            (None, Outcome::Completed) => Verdict::Unchecked,

            // Cut short with nothing checked: there is no telling what
            // of it was right.
            (None, _) => return,
        };

        // A run that failed says nothing about where the answer was.
        if verdict != Verdict::Failed {
            for (neuron, strength) in std::mem::take(&mut self.run.foreseen) {
                let right = self.run.changed.contains(&neuron);

                self.network.calibration_mut().add(strength, right);
            }
        }

        let evidence = std::mem::take(&mut self.run.evidence);

        self.network.learn(&evidence, verdict);
        self.run = Run::default();
    }

    fn hunches(&self) -> Vec<Hunch> {
        if self.network.runs() < MIN_RUNS {
            return Vec::new();
        }

        let words = self.run.words.iter().map(|word| (*word, 1.0));
        let fading = self
            .run
            .trace
            .iter()
            .map(|(neuron, trace)| (*neuron, *trace));

        let sources: Vec<(u32, f32)> = words.chain(fading).collect();

        let recalled: Vec<(u32, f32)> = self
            .network
            .recall(&sources)
            .into_iter()
            .filter(|(neuron, _)| {
                !self.run.visited.contains(neuron) && !self.run.mentioned.contains(neuron)
            })
            .collect();

        let Some(&(_, strongest)) = recalled.first() else {
            return Vec::new();
        };

        let threshold = self.network.calibration().threshold(PRECISION, THRESHOLD);

        recalled
            .into_iter()
            .take_while(|(_, strength)| {
                *strength >= threshold && *strength >= strongest * RIVAL_SHARE
            })
            .take(MENTIONED)
            .map(|(neuron, strength)| Hunch {
                concept: self.network.concept(neuron).clone(),
                strength,
                because: self.because(neuron, &sources),
            })
            .collect()
    }

    /// The three active concepts with the strongest synapses onto
    /// `neuron`.
    fn because(&self, neuron: u32, sources: &[(u32, f32)]) -> Vec<String> {
        let mut support: Vec<(f32, u32)> = sources
            .iter()
            .map(|(source, activity)| (activity * self.network.weight(*source, neuron), *source))
            .filter(|(support, _)| *support > 0.0)
            .collect();

        support.sort_by(|left, right| right.0.total_cmp(&left.0));

        support
            .into_iter()
            .take(3)
            .map(|(_, source)| self.network.concept(source).text.clone())
            .collect()
    }

    fn mention(&mut self, hunches: &[Hunch]) {
        for hunch in hunches {
            if let Some(neuron) = self.network.find(&hunch.concept) {
                self.run.mentioned.insert(neuron);
            }
        }
    }
}

impl TrajectorySink for Intuition {
    fn record(&self, event: &Event) {
        let mut state = self.state();

        match event {
            Event::UserInterjected { text, .. } => {
                for word in concept::task_words(text) {
                    let id = state.network.neuron(Concept::new(Kind::Word, word));
                    if !state.run.words.contains(&id) {
                        state.run.words.push(id);
                    }
                }
            }
            Event::RunStarted { instruction, .. } => {
                if let Some(file) = &self.file {
                    state.network = Network::load_or_new(file);
                }
                state.start(instruction);
            }

            Event::ModelResponded { step, .. } => state.next_step(*step),

            Event::ToolStarted { call, .. } => {
                state.run.pending.insert(call.id.clone(), call.clone());
            }

            Event::ToolFinished { result, .. } => {
                if let Some(call) = state.run.pending.remove(&result.call_id) {
                    state.tool_finished(&call, &result.output, result.is_error);
                }
            }

            Event::VerificationRan { step, checks, .. } => {
                for check in checks {
                    let commands = [check.command.clone()];

                    state.ran(&commands, check.passed, &check.output, *step);
                }
            }

            Event::RunFinished {
                outcome, verified, ..
            } => {
                let _lock = match self
                    .file
                    .as_ref()
                    .map(|file| crate::persistence::lock(file))
                    .transpose()
                {
                    Ok(lock) => lock,
                    Err(error) => {
                        eprintln!("warning: cannot lock intuition learning: {error}");
                        return;
                    }
                };
                if let Some(file) = &self.file {
                    state.rebase(Network::load_or_new(file));
                }
                state.finish(*outcome, *verified);

                self.save(&state);
            }

            _ => {}
        }
    }
}

impl Adviser for Intuition {
    fn telemetry(&self) -> Option<Value> {
        let state = self.state();
        let active: Vec<Value> = state
            .run
            .words
            .iter()
            .map(|id| {
                json!({
                    "concept": state.network.concept(*id), "activity": 1.0,
                })
            })
            .chain(state.run.trace.iter().map(|(id, activity)| {
                json!({
                    "concept": state.network.concept(*id), "activity": activity,
                })
            }))
            .take(96)
            .collect();
        Some(json!({
            "source": SOURCE, "picture": state.network.picture(60),
            "active": active, "hunches": detail(&state.hunches()),
        }))
    }
    fn advise(&self, step: u32, _max_steps: u32) -> Option<Advice> {
        let mut state = self.state();

        let hunches = state.hunches();
        let reminder = state.run.loops.reminder(step);

        if hunches.is_empty() && reminder.is_none() {
            return None;
        }

        state.mention(&hunches);

        let mut sentences: Vec<String> = Vec::new();

        if !hunches.is_empty() {
            sentences.push(format!(
                "From a state like this one, earlier completed runs went on to {} (unchecked runs carry half weight).",
                phrase(&hunches)
            ));
        }

        sentences.extend(reminder.clone());

        let text = format!(
            "[{SOURCE}, from {} earlier runs in this repository] {} A reminder, not an \
             instruction.",
            state.network.runs(),
            sentences.join(" "),
        );

        Some(Advice {
            source: SOURCE.to_string(),
            text,
            detail: json!({
                "hunches": detail(&hunches),
                "reminder": reminder,
                "runs": state.network.runs(),
            }),
        })
    }
}

#[async_trait]
impl ContextSource for Intuition {
    async fn brief(&self, instruction: &str, workspace: &Workspace) -> Option<Briefing> {
        let mut state = self.state();

        // In a run the task's words are already active. Asked outside one
        // (to preview what a task would be given), take them from here.
        if state.run.words.is_empty() {
            state.start(instruction);
        }

        // Only files can be named before anything has happened, and only
        // ones that are still there.
        let hunches: Vec<Hunch> = state
            .hunches()
            .into_iter()
            .filter(|hunch| {
                hunch.concept.kind == Kind::File
                    && workspace.root().join(&hunch.concept.text).is_file()
            })
            .collect();

        if hunches.is_empty() {
            return None;
        }

        state.mention(&hunches);

        let mut text = format!(
            "## Hunch\n\nFrom {} earlier runs in this repository: the completed ones on tasks worded \
             like this one changed:\n\n",
            state.network.runs()
        );

        for hunch in &hunches {
            text.push_str(&format!(
                "- `{}` ({:.2})\n",
                hunch.concept.text, hunch.strength
            ));
        }

        text.push_str("\nAn association, not a finding: read before relying on it.");

        Some(Briefing {
            source: SOURCE.to_string(),
            text,
            detail: json!({
                "hunches": detail(&hunches),
                "runs": state.network.runs(),
            }),
        })
    }
}

/// "open `a.ts` (0.82) and `b.ts` (0.61), and run `npm install` (0.70)".
fn phrase(hunches: &[Hunch]) -> String {
    let named = |kind: Kind| -> Vec<String> {
        hunches
            .iter()
            .filter(|hunch| hunch.concept.kind == kind)
            .map(|hunch| format!("`{}` ({:.2})", hunch.concept.text, hunch.strength))
            .collect()
    };

    let files = named(Kind::File);
    let commands = named(Kind::Command);

    let mut parts = Vec::new();

    if !files.is_empty() {
        parts.push(format!("change {}", files.join(", ")));
    }

    if !commands.is_empty() {
        parts.push(format!("run {}", commands.join(", ")));
    }

    parts.join(" and ")
}

fn detail(hunches: &[Hunch]) -> Value {
    hunches
        .iter()
        .map(|hunch| {
            json!({
                "kind": hunch.concept.kind.label(),
                "text": hunch.concept.text,
                "strength": hunch.strength,
                "because": hunch.because,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn concurrent_chats_merge_concepts_and_learning() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("intuition.json");
        let chats: Vec<_> = (0..12)
            .map(|_| Intuition::new(Some(path.clone())))
            .collect();
        std::thread::scope(|scope| {
            for (index, chat) in chats.into_iter().enumerate() {
                scope.spawn(move || {
                    let file = format!("src/task{index}.rs");
                    solve(&chat, "repair shared calendar", &[], &file);
                    assert!(chat.telemetry().is_some());
                });
            }
        });
        let network = Network::load_or_new(&path);
        assert_eq!(network.runs(), 12);
        for index in 0..12 {
            assert!(
                network
                    .find(&Concept::new(Kind::File, format!("src/task{index}.rs")))
                    .is_some()
            );
        }
    }
    use cortex_core::{ToolResult, Usage};

    use super::*;

    fn start(intuition: &Intuition, instruction: &str) {
        intuition.record(&Event::RunStarted {
            instruction: instruction.to_string(),
            provider: "scripted".to_string(),
            model: "scripted".to_string(),
        });
    }

    /// One step with one tool call.
    fn step(intuition: &Intuition, step: u32, name: &str, input: Value, output: &str) {
        let (message, stop_reason) = crate::situation::tests::response(&[(name, input.clone())]);

        intuition.record(&Event::ModelResponded {
            step,
            message,
            stop_reason,
            usage: Usage::default(),
            duration_ms: 1,
        });

        intuition.record(&Event::ToolStarted {
            step,
            call: ToolCall {
                id: "call".to_string(),
                name: name.to_string(),
                input,
            },
        });

        intuition.record(&Event::ToolFinished {
            step,
            tool: name.to_string(),
            result: ToolResult {
                call_id: "call".to_string(),
                output: output.to_string(),
                is_error: false,
            },
            duration_ms: 1,
        });
    }

    fn read(intuition: &Intuition, at: u32, path: &str) {
        step(intuition, at, "read_file", json!({ "path": path }), "...");
    }

    fn edit(intuition: &Intuition, at: u32, path: &str) {
        step(
            intuition,
            at,
            "edit_file",
            json!({ "path": path }),
            "edited",
        );
    }

    fn shell(intuition: &Intuition, at: u32, command: &str, output: &str) {
        step(
            intuition,
            at,
            "shell",
            json!({ "command": command }),
            output,
        );
    }

    fn finish(intuition: &Intuition, verified: Option<bool>) {
        intuition.record(&Event::RunFinished {
            outcome: Outcome::Completed,
            steps: 1,
            usage: Usage::default(),
            duration_ms: 1,
            verified,
        });
    }

    /// A passing run that reads `reads`, then changes `changed`.
    fn solve(intuition: &Intuition, instruction: &str, reads: &[&str], changed: &str) {
        start(intuition, instruction);

        let mut at = 1;

        for path in reads {
            read(intuition, at, path);

            at += 1;
        }

        edit(intuition, at, changed);
        shell(intuition, at + 1, "npm test", "exit code: 0");
        finish(intuition, Some(true));
    }

    /// Four kinds of task, each ending in its own file, and every one
    /// reading `package.json` on the way.
    fn experienced() -> Intuition {
        let intuition = Intuition::new(None);

        let kinds = [
            (
                "The calendar drops week {} of the month view",
                "src/calendar/calendar.ts",
            ),
            (
                "Imported fills lose commission {} on partial exits",
                "src/imports/matching.ts",
            ),
            (
                "Journal entries reject legacy tags {}",
                "src/journal/schema.ts",
            ),
            (
                "Negative money amounts display wrongly {}",
                "src/lib/money.ts",
            ),
        ];

        for round in 0..3 {
            for (instruction, changed) in kinds {
                solve(
                    &intuition,
                    &instruction.replace("{}", &round.to_string()),
                    &["package.json", "src/shared/view.tsx"],
                    changed,
                );
            }
        }

        intuition
    }

    /// A task worded much like the calendar tasks met before.
    const CALENDAR_AGAIN: &str = "The calendar month view drops the last week";

    fn names(hunches: &[Hunch]) -> Vec<&str> {
        hunches
            .iter()
            .map(|hunch| hunch.concept.text.as_str())
            .collect()
    }

    #[test]
    fn a_new_task_brings_to_mind_where_tasks_like_it_ended() {
        let intuition = experienced();

        start(&intuition, CALENDAR_AGAIN);

        let hunches = intuition.hunches();

        assert_eq!(names(&hunches)[0], "src/calendar/calendar.ts");
        assert!(!names(&hunches).contains(&"src/imports/matching.ts"));

        // The file every task reads tells nothing, so it is not offered.
        assert!(!names(&hunches).contains(&"package.json"));

        assert!(hunches[0].because.contains(&"calendar".to_string()));
    }

    #[test]
    fn nothing_is_said_about_a_task_unlike_any_before() {
        let intuition = experienced();

        start(&intuition, "Rename the settings page heading");

        assert!(intuition.hunches().is_empty());
        assert!(intuition.advise(1, 60).is_none());

        // One word in common with the calendar tasks is a coincidence.
        start(
            &intuition,
            "Add a dark theme toggle to the settings view of the journal page",
        );

        assert!(intuition.hunches().is_empty());
    }

    #[test]
    fn an_inexperienced_network_is_silent() {
        let intuition = Intuition::new(None);

        solve(
            &intuition,
            "The calendar drops a week",
            &[],
            "src/calendar/calendar.ts",
        );

        start(&intuition, "The calendar drops a week");

        assert!(intuition.hunches().is_empty());
    }

    #[test]
    fn a_hunch_is_said_once_and_never_about_a_file_already_open() {
        let intuition = experienced();

        start(&intuition, CALENDAR_AGAIN);
        read(&intuition, 1, "README.md");

        let advice = intuition.advise(1, 60).expect("a hunch");

        assert!(
            advice.text.contains("`src/calendar/calendar.ts`"),
            "{}",
            advice.text
        );
        assert_eq!(advice.detail["hunches"][0]["kind"], "file");

        assert!(intuition.advise(2, 60).is_none(), "said already");

        // In a new run, opening the file first leaves nothing to say
        // about it.
        start(&intuition, CALENDAR_AGAIN);
        read(&intuition, 1, "src/calendar/calendar.ts");

        assert!(!names(&intuition.hunches()).contains(&"src/calendar/calendar.ts"));
    }

    #[test]
    fn an_error_brings_to_mind_what_was_run_after_it_before() {
        // Other runs never met the error, so it is telling when it comes.
        let intuition = experienced();

        for round in 0..4 {
            start(&intuition, &format!("Upgrade dependency number {round}"));

            shell(
                &intuition,
                1,
                "npm test",
                "exit code: 1\n\nstderr:\nError: Cannot find module 'luxon'",
            );

            shell(&intuition, 2, "npm install", "exit code: 0");
            shell(&intuition, 3, "npm test", "exit code: 0");
            finish(&intuition, Some(true));
        }

        start(&intuition, "Something else entirely");

        assert!(intuition.hunches().is_empty());

        shell(
            &intuition,
            1,
            "npm test",
            "exit code: 1\n\nstderr:\nError: Cannot find module 'luxon'",
        );

        let advice = intuition.advise(1, 60).expect("a hunch");

        assert!(advice.text.contains("run `npm install`"), "{}", advice.text);
    }

    #[test]
    fn the_task_still_weighs_on_recall_late_in_a_long_run() {
        let intuition = experienced();

        start(&intuition, CALENDAR_AGAIN);

        for at in 1..=50 {
            read(&intuition, at, &format!("docs/page{at}.md"));
        }

        assert_eq!(names(&intuition.hunches())[0], "src/calendar/calendar.ts");
    }

    #[test]
    fn a_long_run_is_reminded_of_a_check_it_left_failing() {
        let intuition = Intuition::new(None);

        start(&intuition, "Fix the importer");

        shell(
            &intuition,
            1,
            "npm test",
            "exit code: 1\n\nstdout:\n1 failed",
        );
        edit(&intuition, 2, "src/a.ts");

        for at in 3..=5 {
            read(&intuition, at, "src/b.ts");

            assert!(intuition.advise(at, 60).is_none());
        }

        read(&intuition, 6, "src/c.ts");

        let advice = intuition.advise(6, 60).expect("a reminder");

        assert!(
            advice.text.contains("`npm test` failed at step 1"),
            "{}",
            advice.text
        );
    }

    #[test]
    fn a_failed_run_teaches_no_new_association() {
        let intuition = Intuition::new(None);

        for _ in 0..5 {
            start(&intuition, "The calendar drops a week");
            edit(&intuition, 1, "src/wrong.ts");
            finish(&intuition, Some(false));
        }

        assert_eq!(intuition.runs(), 5);
        assert_eq!(intuition.synapses(), 0);
    }

    #[test]
    fn what_was_learned_is_kept_between_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("memory.intuition.json");

        let first = Intuition::new(Some(file.clone()));

        solve(
            &first,
            "The calendar drops a week",
            &[],
            "src/calendar/calendar.ts",
        );

        let second = Intuition::new(Some(file));

        assert_eq!(second.runs(), 1);
        assert!(second.synapses() > 0);
    }

    #[tokio::test]
    async fn the_briefing_names_only_files_that_are_still_there() {
        let intuition = experienced();

        let dir = tempfile::tempdir().unwrap();
        let workspace = Workspace::new(dir.path()).unwrap();

        let instruction = CALENDAR_AGAIN;

        start(&intuition, instruction);

        assert!(intuition.brief(instruction, &workspace).await.is_none());

        std::fs::create_dir_all(dir.path().join("src/calendar")).unwrap();
        std::fs::write(dir.path().join("src/calendar/calendar.ts"), "").unwrap();

        start(&intuition, instruction);

        let briefing = intuition.brief(instruction, &workspace).await.unwrap();

        assert!(briefing.text.contains("`src/calendar/calendar.ts`"));

        // Said before the task, so not said again after the first step.
        read(&intuition, 1, "README.md");

        assert!(intuition.advise(1, 60).is_none());
    }
}
