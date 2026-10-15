//! The brain: one cortex for everything it learns.
//!
//! - [`words`]: how each thing reaches the cortex. A word, or a percept
//!   such as `does: edit_file` or `move: up`, fires a sparse set of
//!   columns; things met alike come to share some.
//! - [`cortex`]: columns of spiking cells whose dendritic branches learn the
//!   context before each thing, so that the right cells are primed for
//!   what comes next. It grows branches, synapses and cells, and prunes
//!   them. How readily its synapses change rises with surprise and news,
//!   as neuromodulators make it.
//! - [`engram`]: each thing learned or lived is kept as the cells that held
//!   it (the index the hippocampus is thought to keep), and recalled by
//!   re-firing them.
//! - [`value`]: what matters, given and learned, and what is held in mind.
//! - [`mind`]: what it keeps about itself: time as it felt, episodes, how
//!   far its sureness can be trusted, its thoughts, and how its advice
//!   has gone.
//! - [`map`]: where each column sits on the drawn cortex.
//!
//! [`Brain`] ties them together, and its work is split by what it is doing:
//!
//! - learning and hearing text, here;
//! - [`life`]: living streams of experience (an agent's work, a market, a
//!   chat), with value, attention, episodes, advice and plans;
//! - [`speech`]: speaking, a word at a time, weighing near rivals by where
//!   each leads before committing;
//! - [`recall`]: remembering from a cue;
//! - [`thought`]: thinking, imagining, reasoning, intuition, reflection;
//! - [`sleep`]: replaying memories (those most in need first), pruning,
//!   and dreaming;
//! - [`view`] and [`store`]: what it shows of itself, and keeping it on
//!   disk.
//!
//! The neural component is a contextual sequence learner. [`cognition`]
//! routes conversation through that learner and explicit skills:
//! [`arithmetic`], [`quantity`], [`knowledge`], and [`formula`]. Explanations
//! retain the executed operations and source premises. [`learning`]
//! distinguishes lesson retention from frozen evaluation. These are
//! limited functional mechanisms, not evidence of subjective experience.

pub mod arithmetic;
pub mod cognition;
pub mod cortex;
pub mod engram;
pub mod formula;
pub mod knowledge;
pub mod learning;
pub mod life;
pub mod map;
pub mod mind;
pub mod quantity;
pub mod recall;
pub mod sleep;
pub mod speech;
pub mod store;
pub mod thought;
pub mod value;
pub mod view;
pub mod words;

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

pub use cortex::{Activation, BranchFire, Changes, Cortex, Totals};
pub use engram::{Engram, Recall, join};
pub use life::{Hunch, HunchKind, Imagined, Moment, Outlook};
pub use mind::{
    Activity, AdviceRecord, Episode, Felt, Mind, MindView, Note, PROCESSES, SelfModel, SenseMeters,
    Thought, Wiring,
};
pub use store::mind_file;
pub use value::{Attention, Values};
pub use view::{columns_of, heat};
use words::{ACTIVE, BRAIN, STORY, USER, Words, ends_sentence, tokenize};

/// Steps kept for the metrics view.
const TRACE: usize = 240;
const RECALLS: usize = 24;
const SAID: usize = 24;

/// Words it says at most in one go, and thinks or dreams.
const LONGEST: usize = 40;
const DREAMED: usize = 24;

/// Steps of activity kept for the view, and how far back it looks.
const ACTIVITY: usize = 6_000;
const ACTIVITY_WINDOW_MS: u64 = 8_000;

/// Memories whose columns the snapshot carries; the rest come with the
/// wiring.
const ENGRAM_COLUMNS: usize = 80;

/// Continuations reasoning keeps open at each depth, and tries from each.
const BEAM: usize = 4;
const BRANCHING: usize = 4;

/// Trades of place the map tries each sleep cycle.
const ORGANISE: usize = 3_000;

/// Steps unused before a branch fades during sleep, and how much.
const IDLE: u64 = 30_000;
const FADING: f32 = 0.97;

/// Cue cells an engram must share for recall to choose it.
const RECOGNISED: f32 = 0.05;

/// Words per column on average beyond which the cortex grows, and by how
/// many columns at a time.
const CROWDED: usize = 30;
const GROWTH: usize = 1024;

/// The memory the brain may grow into when nobody has said otherwise.
pub const DEFAULT_BUDGET: u64 = 6 << 30;

/// One step of the brain, with why.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub number: u64,
    pub time_ms: u64,

    /// learn, hear, speak, recall, replay.
    pub mode: String,
    pub token: String,

    /// Of the word's columns: how many had a primed cell, and how many
    /// burst because none did.
    pub predicted: u32,
    pub bursting: u32,
    pub surprise: f32,
    pub spiking: u32,

    /// What it expected before the word, best first: word, share of its
    /// columns primed, columns primed.
    pub guesses: Vec<(String, f32, u32)>,
    pub chosen: Option<String>,
    pub why: String,

    /// The strongest dendritic spikes this step set off, priming the next.
    pub branches: Vec<BranchFire>,
    pub changes: Changes,
}

/// Something it said, and whether it was taught that sentence.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Utterance {
    pub time_ms: u64,
    pub prompt: String,
    pub text: String,

    /// It contains a sentence of three or more words that it never learned.
    pub novel: bool,
    pub new_sentences: Vec<String>,

    /// The mean share of each chosen word's columns that were primed.
    pub confidence: f32,
    pub why: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Stats {
    pub lessons: u64,
    pub tokens_learned: u64,

    /// Fresh words it was taught, how many it predicted first, and how
    /// many always guessing the commonest word would have.
    pub scored: u64,
    pub right: u64,
    pub baseline_right: u64,
    pub recent_right: f32,
    pub recent_baseline: f32,

    pub replays: u64,
    pub sleeps: u64,
    pub drifted: u64,
    pub spoken: u64,
    pub novel: u64,
    pub recalls: u64,
    pub recall_fidelity: f32,
    pub engrams_forgotten: u64,
    pub corrections: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EngramView {
    pub id: u64,
    pub text: String,
    pub source: String,
    pub strength: f32,
    pub recalls: u64,
    pub replays: u64,
    pub cells: u32,
    pub columns: Vec<u32>,
    pub fidelity: Option<f32>,
    pub created: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Curiosity {
    pub word: String,
    pub surprise: f32,
    pub progress: f32,
    pub seen: u64,
}

/// The brain as it is now, for the network view.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NeuralSnapshot {
    pub stats: Stats,
    pub totals: Totals,
    pub clock: u64,
    pub vocabulary: usize,
    pub cells: usize,
    pub branches: usize,
    pub synapses: usize,
    pub connected: usize,
    pub permanences: [u32; 10],

    /// Per column: cells, branches.
    pub columns: Vec<(u32, u32)>,

    /// The columns that fired in the latest steps, oldest first, and the
    /// columns primed now.
    pub recent: Vec<Vec<u32>>,
    pub primed: Vec<u32>,

    pub engrams: Vec<EngramView>,
    pub engram_count: usize,
    pub trace: Vec<Step>,
    pub recalls: Vec<Recall>,
    pub said: Vec<Utterance>,
    pub curious: Vec<Curiosity>,
    pub phase: String,

    /// Columns now, bytes held, and the budget it may grow into.
    pub area: usize,
    pub footprint: u64,
    pub budget: u64,

    pub mind: MindView,
    pub cognition: cognition::CognitiveView,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Brain {
    pub words: Words,
    pub cortex: Cortex,
    engrams: VecDeque<Engram>,
    next_engram: u64,
    sentences: HashSet<u64>,
    stats: Stats,
    trace: VecDeque<Step>,
    recalls: VecDeque<Recall>,
    said: VecDeque<Utterance>,
    steps: u64,
    random: u64,
    phase: String,
    #[serde(skip)]
    recent: VecDeque<Vec<u32>>,
    #[serde(skip)]
    budget: u64,

    /// Kept in a file of its own beside the cortex's, so that saved
    /// brains from before it still load.
    #[serde(skip)]
    pub mind: Mind,
    #[serde(skip)]
    activity: VecDeque<Activity>,
    #[serde(skip)]
    slots: Vec<(f32, f32)>,

    /// The streams of experience being lived, each with where the cortex
    /// was in it.
    #[serde(skip)]
    streams: HashMap<String, life::Stream>,

    /// Surprise averaged over the last few steps: above the long average,
    /// things have changed and it learns faster.
    #[serde(skip)]
    fast_surprise: f32,
}

/// A continuation reasoning keeps open: where the cortex was, the path
/// there, and its score.
type Open = (cortex::State, Vec<(String, f32)>, f32);

/// A run of words the primed cells chose.
pub(crate) struct Chain {
    pub(crate) words: Vec<String>,
    pub(crate) confidence: f32,
    pub(crate) reasons: Vec<String>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn sentence_hash(tokens: &[String]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325_u64;
    for token in tokens {
        for b in token.bytes().chain([0]) {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

/// The sentences of a token run: words between sentence ends, without
/// turn marks.
fn sentences(tokens: &[String]) -> Vec<Vec<String>> {
    let mut all = Vec::new();
    let mut current = Vec::new();
    for token in tokens {
        if matches!(token.as_str(), USER | BRAIN | STORY) || ends_sentence(token) {
            if !current.is_empty() {
                all.push(std::mem::take(&mut current));
            }
            continue;
        }
        if token != "," {
            current.push(token.clone());
        }
    }
    if !current.is_empty() {
        all.push(current);
    }
    all
}

impl Brain {
    pub fn new() -> Self {
        Self {
            random: 0x9E37_79B9_7F4A_7C15,
            phase: "newly made".into(),
            ..Self::default()
        }
    }

    /// The columns a word fires, coding a new word over the cortex's area.
    fn code(&mut self, token: &str) -> Option<Vec<u32>> {
        let area = self.cortex.area();
        self.words.columns(token, area)
    }

    /// Roughly how many bytes the brain holds: cortex, memories, words.
    pub fn footprint(&self) -> u64 {
        let memories: usize = self
            .engrams
            .iter()
            .map(|e| {
                e.tokens.iter().map(|t| t.len() + 24).sum::<usize>()
                    + e.cells.iter().map(|c| c.len() * 4 + 24).sum::<usize>()
            })
            .sum();
        self.cortex.footprint()
            + memories as u64
            + self.words.len() as u64 * 400
            + self.mind.cognition.footprint()
    }

    /// The memory the brain may grow into, in bytes.
    pub fn budget(&self) -> u64 {
        if self.budget > 0 {
            self.budget
        } else {
            DEFAULT_BUDGET
        }
    }

    pub fn set_budget(&mut self, bytes: u64) {
        self.budget = bytes.max(64 << 20);
    }

    /// Grows the cortex when its columns are crowded: each column shared by
    /// more than [`CROWDED`] words on average. Returns columns grown.
    fn grow_area(&mut self) -> usize {
        let area = self.cortex.area();
        if self.words.len() * words::ACTIVE <= area * CROWDED {
            return 0;
        }
        self.cortex.grow_columns(GROWTH);
        self.mind.map.fit(self.cortex.area());
        self.phase = format!(
            "grew {GROWTH} columns: {} words were crowding {area}",
            self.words.len()
        );
        GROWTH
    }

    /// Past its budget, the weakest memories are forgotten and unused
    /// synapses pruned harder, until it fits. Returns memories forgotten.
    fn keep_within_budget(&mut self) -> usize {
        let mut forgotten = 0;
        let mut rounds = 0;
        while self.footprint() > self.budget() && rounds < 20 {
            rounds += 1;
            let drop = (self.engrams.len() / 20).max(1).min(self.engrams.len());
            let mut order: Vec<(usize, f32)> = self
                .engrams
                .iter()
                .enumerate()
                .map(|(at, e)| (at, e.strength))
                .collect();
            order.sort_by(|a, b| a.1.total_cmp(&b.1));
            let gone: HashSet<usize> = order.into_iter().take(drop).map(|(at, _)| at).collect();
            let mut at = 0;
            self.engrams.retain(|_| {
                let keep = !gone.contains(&at);
                at += 1;
                keep
            });
            forgotten += gone.len();
            let (_, _, died) = self.cortex.prune(IDLE / 4, 0.8);
            self.forget_cells(&died);
        }
        self.mind
            .notes
            .retain(|id, _| self.engrams.iter().any(|e| e.id == *id));
        if forgotten > 0 {
            let kept: HashSet<u64> = self.engrams.iter().map(|e| e.id).collect();
            let knowledge = &mut self.mind.cognition.knowledge;
            knowledge
                .statements
                .retain(|fact| kept.contains(&fact.memory));
            knowledge
                .dialogue
                .retain(|dialogue| kept.contains(&dialogue.memory));
            knowledge
                .formulas
                .retain(|formula| kept.contains(&formula.memory));
        }
        self.stats.engrams_forgotten += forgotten as u64;
        forgotten
    }

    fn draw(&mut self) -> u64 {
        if self.random == 0 {
            self.random = 0x9E37_79B9_7F4A_7C15;
        }
        self.random ^= self.random << 13;
        self.random ^= self.random >> 7;
        self.random ^= self.random << 17;
        self.random
    }

    pub fn is_sound(&self) -> bool {
        self.cortex.is_sound()
    }

    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    fn record(&mut self, step: Step) {
        self.trace.push_back(step);
        while self.trace.len() > TRACE {
            self.trace.pop_front();
        }
    }

    fn remember_columns(&mut self, columns: &[u32]) {
        self.recent.push_back(columns.to_vec());
        while self.recent.len() > 16 {
            self.recent.pop_front();
        }
    }

    /// One word reaches the cortex as `process`: it steps, and the step is
    /// lived: felt, attended to, and kept for the view with its cells.
    fn fire(&mut self, process: &str, token: &str, columns: &[u32], learn: bool) -> Activation {
        if learn {
            self.cortex.plasticity = self.plasticity(token, process);
        }
        let activation = self.cortex.step(columns, learn);
        self.remember_columns(columns);
        let changes = &activation.changes;
        let learned = learn
            && changes.strengthened
                + changes.synapses_grown
                + changes.segments_grown
                + changes.cells_grown
                > 0;
        let surprise = activation.bursting as f32 / columns.len().max(1) as f32;
        self.lived(
            process,
            token,
            activation.spiking.clone(),
            surprise,
            learned,
        );
        activation
    }

    /// Fires a word by its text; `None` for one that has no code.
    pub(crate) fn fire_word(&mut self, process: &str, token: &str) -> Option<Activation> {
        let columns = self.code(token)?;
        Some(self.fire(process, token, &columns, false))
    }

    /// Cells set spiking without a word, as recall and dreams re-fire a
    /// memory's cells.
    fn reignite(&mut self, process: &str, token: &str, cells: &[u32]) {
        self.cortex.reactivate(cells);
        self.lived(process, token, cells.to_vec(), 0.0, false);
    }

    /// How readily synapses change for this step. Things have changed
    /// when it has lately been more surprised than it usually is, and then
    /// it learns faster; news (good or bad) makes what comes with it
    /// stick; replay in sleep is gentler than waking life. From 0.5 to 1.6.
    fn plasticity(&self, token: &str, process: &str) -> f32 {
        let change = (self.fast_surprise - self.mind.me.surprise).clamp(-0.4, 0.4);
        let news = self
            .mind
            .values
            .meanings
            .get(token)
            .map_or(0.0, |worth| worth.abs());
        let base = if process == "replay" { 0.8 } else { 1.0 };
        (base + change + 0.4 * news).clamp(0.5, 1.6)
    }

    fn lived(&mut self, process: &str, token: &str, cells: Vec<u32>, surprise: f32, learned: bool) {
        let now = now_ms();
        // Only what is met counts towards how surprised it usually is,
        // which sets how readily it learns: imagining and remembering
        // must leave learning as they found it.
        let perceived = matches!(process, "learn" | "hear" | "live");
        if perceived {
            self.fast_surprise += 0.3 * (surprise - self.fast_surprise);
        }
        self.mind.felt.tick(process, token, surprise, learned, now);
        let me = &mut self.mind.me;
        if perceived {
            me.surprise += 0.05 * (surprise - me.surprise);
        }
        me.focus = self.mind.felt.present.iter().cloned().collect();
        me.doing = process.into();
        let mut primed: Vec<u32> = self.cortex.primed_cells().into_iter().collect();
        primed.sort_unstable();
        self.activity.push_back(Activity {
            number: self.cortex.clock,
            time_ms: now,
            process: process.into(),
            word: token.into(),
            cells,
            primed,
            learned,
        });
        while self.activity.len() > ACTIVITY {
            self.activity.pop_front();
        }
    }

    /// The commonest word, not counting turn marks or percepts: what the
    /// baseline guesses every time.
    fn commonest(&self) -> Option<String> {
        self.words
            .words
            .iter()
            .filter(|(t, _)| !matches!(t.as_str(), USER | BRAIN | STORY) && !t.contains(": "))
            .max_by(|a, b| a.1.seen.cmp(&b.1.seen).then(b.0.cmp(a.0)))
            .map(|(t, _)| t.clone())
    }

    pub(crate) fn guesses(&self, most: usize) -> Vec<(String, f32, u32)> {
        self.words
            .read(&self.cortex.primed_columns(), most)
            .into_iter()
            .map(|(word, share, hits)| (word, share, hits as u32))
            .collect()
    }

    /// Runs tokens through the cortex. `mode` is `learn` for something
    /// taught (scored fresh), `hear` for something heard (learned, scored),
    /// `replay` for its own memory in sleep (learned, not scored). Returns
    /// the winner cells of each step.
    fn run(&mut self, tokens: &[String], mode: &str) -> Vec<Vec<u32>> {
        let learn = true;
        // Only text it has not been taught before tests it: the same lesson
        // again, or a correction's second pass, would score what it
        // remembers as if it had foreseen it.
        let scored = mode != "replay" && !self.engrams.iter().any(|e| e.tokens == tokens);
        let heard = mode != "replay";
        self.cortex.reset();
        let commonest = self.commonest();
        let mut cells = Vec::new();
        for (at, token) in tokens.iter().enumerate() {
            let Some(columns) = self.code(token) else {
                continue;
            };
            let guesses = if at > 0 { self.guesses(5) } else { Vec::new() };
            let right = guesses.first().is_some_and(|g| &g.0 == token);
            // How far its sureness holds counts what it remembers too: being
            // right when sure is what sureness is for.
            if heard && let Some(top) = guesses.first() {
                self.mind.me.guessed(top.1, right);
            }
            if scored && at > 0 {
                let baseline = commonest.as_ref() == Some(token);
                self.stats.scored += 1;
                self.stats.right += u64::from(right);
                self.stats.baseline_right += u64::from(baseline);
                self.stats.recent_right +=
                    0.02 * (f32::from(u8::from(right)) - self.stats.recent_right);
                self.stats.recent_baseline +=
                    0.02 * (f32::from(u8::from(baseline)) - self.stats.recent_baseline);
            }
            let new = self.words.words.get(token).is_none_or(|w| w.seen == 0);
            let activation = self.fire(mode, token, &columns, learn);
            let surprise = activation.bursting as f32 / ACTIVE as f32;
            if heard {
                let neighbours: Vec<&str> = tokens
                    [at.saturating_sub(2)..(at + 3).min(tokens.len())]
                    .iter()
                    .map(String::as_str)
                    .collect();
                self.words.met(token, &neighbours, surprise);
                self.stats.tokens_learned += 1;
                if token.chars().any(char::is_alphanumeric)
                    && !matches!(token.as_str(), USER | BRAIN | STORY)
                {
                    self.mind
                        .attention
                        .met(token, 0.2 + surprise + if new { 1.0 } else { 0.0 });
                }
            }
            let why = if at == 0 {
                "first word: nothing came before it, so every column burst and a cell in each began learning".to_string()
            } else if activation.bursting == 0 {
                format!(
                    "expected: all {} of its columns had a primed cell; those branches were strengthened",
                    columns.len()
                )
            } else if activation.predicted == 0 {
                format!(
                    "surprise: none of its {} columns was primed; all burst, and a cell in each grew a branch towards what came before",
                    columns.len()
                )
            } else {
                format!(
                    "partly expected: {} of {} columns primed, {} burst and learned the context",
                    activation.predicted,
                    columns.len(),
                    activation.bursting
                )
            };
            self.steps += 1;
            let step = Step {
                number: self.steps,
                time_ms: now_ms(),
                mode: mode.into(),
                token: token.clone(),
                predicted: activation.predicted,
                bursting: activation.bursting,
                surprise,
                spiking: activation.spiking.len() as u32,
                guesses,
                chosen: None,
                why,
                branches: self.cortex.branch_fires(4),
                changes: activation.changes,
            };
            self.record(step);
            cells.push(activation.winners);
        }
        cells
    }

    /// Learns text it is taught or told, and keeps it as an engram.
    /// Returns the engram's id.
    pub fn learn(&mut self, text: &str, source: &str) -> Option<u64> {
        let tokens = tokenize(text);
        if tokens.len() < 2 {
            return None;
        }
        let mode = if source == "user" { "hear" } else { "learn" };
        let surprise_before = self.cortex.totals.steps;
        let cells = self.run(&tokens, mode);
        let steps = (self.cortex.totals.steps - surprise_before).max(1);
        let surprise = self
            .trace
            .iter()
            .rev()
            .take(steps as usize)
            .map(|step| step.surprise)
            .sum::<f32>()
            / steps as f32;
        for sentence in sentences(&tokens) {
            self.sentences.insert(sentence_hash(&sentence));
        }
        self.stats.lessons += 1;
        self.grow_area();
        // The same thing learned again is the same memory, strengthened and
        // now held by the cells that hold it today.
        if let Some(known) = self.engrams.iter_mut().find(|e| e.tokens == tokens) {
            known.cells = cells;
            known.strength = (known.strength + 0.5).min(5.0);
            self.phase = format!("relearned memory {} from {source}", known.id);
            let id = known.id;
            self.mind.cognition.knowledge.learn(text, source, id, false);
            return Some(id);
        }
        self.next_engram += 1;
        let id = self.next_engram;
        self.mind.notes.insert(
            id,
            Note {
                surprise,
                worth: 0.0,
                context: source.chars().take(80).collect(),
                time_ms: now_ms(),
            },
        );
        self.engrams.push_back(Engram {
            id,
            source: source.chars().take(80).collect(),
            tokens,
            cells,
            created: self.cortex.clock,
            strength: 1.0,
            recalls: 0,
            replays: 0,
            last_recalled: 0,
            fidelity: None,
        });
        self.phase = format!("learned from {source}");
        self.mind.cognition.knowledge.learn(text, source, id, false);
        Some(id)
    }

    /// A correction: learned twice, as something to get right.
    pub fn correct(&mut self, text: &str, source: &str) -> Option<u64> {
        self.stats.corrections += 1;
        self.learn(text, source);
        let id = self.learn(text, source);
        if let Some(id) = id {
            self.mind.cognition.knowledge.learn(text, source, id, true);
        }
        id
    }

    /// Engram ids and their cells, for tests and inspection.
    /// How many memories it holds.
    pub fn memory_count(&self) -> usize {
        self.engrams.len()
    }

    pub fn engram(&self, id: u64) -> Option<&Engram> {
        self.engrams.iter().find(|e| e.id == id)
    }

    /// Cells that died are let go by every memory and stream that held
    /// them: their ids are given to the next cells born, in other columns.
    fn forget_cells(&mut self, died: &[u32]) {
        if died.is_empty() {
            return;
        }
        let died: HashSet<u32> = died.iter().copied().collect();
        for engram in &mut self.engrams {
            for cells in &mut engram.cells {
                cells.retain(|cell| !died.contains(cell));
            }
        }
        for open in self.streams.values_mut() {
            open.forget_cells(&died);
        }
    }
}

#[cfg(test)]
mod cognition_tests;
#[cfg(test)]
mod tests;
