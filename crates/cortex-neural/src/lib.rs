//! A standalone brain, taught rather than attached to an agent.
//!
//! - [`words`]: each word fires a sparse set of columns; words used alike
//!   come to share some.
//! - [`cortex`]: columns of spiking cells whose dendritic branches learn the
//!   context before each word, so that the right cells are primed for what
//!   comes next. It grows branches, synapses and cells, and prunes them.
//! - [`engram`]: each thing learned is kept as the cells that held it, and
//!   recalled by re-firing them.
//!
//! - [`mind`]: what it keeps about itself: time as it felt, episodes,
//!   how far its sureness can be trusted, and its thoughts.
//! - [`map`]: where each column sits on the drawn cortex.
//!
//! [`Brain`] ties them together. Awake, it learns text, speaks by letting
//! its primed cells choose each next word (deciding between near rivals),
//! recalls, follows its first hunch (intuition), searches for a chain of
//! learned steps from one thing to another (reasoning), lets one thing
//! lead to another unprompted (thinking), and splices two memories at a
//! word they share (imagination). Asleep, it replays memories faithfully
//! and learns from them again, prunes, and dreams: free-running fragments
//! of memories that jump from one to another at shared words. Every step
//! any of these takes is recorded with the cells it used, for the view.
//!
//! What this is, plainly: a sequence-learning network that is surprised by
//! what it did not predict and learns from it, with a running measurement
//! of itself. All of the processes above are the same cortex run in
//! different ways; none is a language model, and none of it shows
//! experience.

pub mod cortex;
pub mod engram;
pub mod map;
pub mod mind;
pub mod words;

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

pub use cortex::{Activation, BranchFire, Changes, Cortex, Totals};
pub use engram::{Engram, Recall, join};
pub use mind::{Activity, Episode, Felt, Mind, MindView, PROCESSES, SelfModel, Thought, Wiring};
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
}

/// A continuation reasoning keeps open: where the cortex was, the path
/// there, and its score.
type Open = (cortex::State, Vec<(String, f32)>, f32);

/// A run of words the primed cells chose.
struct Chain {
    words: Vec<String>,
    confidence: f32,
    reasons: Vec<String>,
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
            .map(|e| e.tokens.iter().map(|t| t.len() + 24).sum::<usize>() + e.cells.iter().map(|c| c.len() * 4 + 24).sum::<usize>())
            .sum();
        self.cortex.footprint() + memories as u64 + self.words.len() as u64 * 400
    }

    /// The memory the brain may grow into, in bytes.
    pub fn budget(&self) -> u64 {
        if self.budget > 0 { self.budget } else { DEFAULT_BUDGET }
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
            let mut order: Vec<(usize, f32)> = self.engrams.iter().enumerate().map(|(at, e)| (at, e.strength)).collect();
            order.sort_by(|a, b| a.1.total_cmp(&b.1));
            let gone: HashSet<usize> = order.into_iter().take(drop).map(|(at, _)| at).collect();
            let mut at = 0;
            self.engrams.retain(|_| {
                let keep = !gone.contains(&at);
                at += 1;
                keep
            });
            forgotten += gone.len();
            self.cortex.prune(IDLE / 4, 0.8);
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
        let activation = self.cortex.step(columns, learn);
        self.remember_columns(columns);
        let changes = &activation.changes;
        let learned = learn
            && changes.strengthened + changes.synapses_grown + changes.segments_grown + changes.cells_grown > 0;
        let surprise = activation.bursting as f32 / columns.len().max(1) as f32;
        self.lived(process, token, activation.spiking.clone(), surprise, learned);
        activation
    }

    /// Fires a word by its text; `None` for one that has no code.
    fn fire_word(&mut self, process: &str, token: &str) -> Option<Activation> {
        let columns = self.code(token)?;
        Some(self.fire(process, token, &columns, false))
    }

    /// Cells set spiking without a word, as recall and dreams re-fire a
    /// memory's cells.
    fn reignite(&mut self, process: &str, token: &str, cells: &[u32]) {
        self.cortex.reactivate(cells);
        self.lived(process, token, cells.to_vec(), 0.0, false);
    }

    fn lived(&mut self, process: &str, token: &str, cells: Vec<u32>, surprise: f32, learned: bool) {
        let now = now_ms();
        self.mind.felt.tick(process, token, surprise, learned, now);
        let me = &mut self.mind.me;
        me.surprise += 0.05 * (surprise - me.surprise);
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

    fn commonest(&self) -> Option<String> {
        self.words
            .words
            .iter()
            .filter(|(t, _)| !matches!(t.as_str(), USER | BRAIN | STORY))
            .max_by(|a, b| a.1.seen.cmp(&b.1.seen).then(b.0.cmp(a.0)))
            .map(|(t, _)| t.clone())
    }

    fn guesses(&self, most: usize) -> Vec<(String, f32, u32)> {
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
        let scored = mode != "replay";
        self.cortex.reset();
        let commonest = self.commonest();
        let mut cells = Vec::new();
        for (at, token) in tokens.iter().enumerate() {
            let Some(columns) = self.code(token) else {
                continue;
            };
            let guesses = if at > 0 { self.guesses(5) } else { Vec::new() };
            if scored && at > 0 {
                let right = guesses.first().is_some_and(|g| &g.0 == token);
                if let Some(top) = guesses.first() {
                    self.mind.me.guessed(top.1, right);
                }
                let baseline = commonest.as_ref() == Some(token);
                self.stats.scored += 1;
                self.stats.right += u64::from(right);
                self.stats.baseline_right += u64::from(baseline);
                self.stats.recent_right += 0.02 * (f32::from(u8::from(right)) - self.stats.recent_right);
                self.stats.recent_baseline +=
                    0.02 * (f32::from(u8::from(baseline)) - self.stats.recent_baseline);
            }
            let activation = self.fire(mode, token, &columns, learn);
            let surprise = activation.bursting as f32 / ACTIVE as f32;
            if scored {
                let neighbours: Vec<&str> = tokens[at.saturating_sub(2)..(at + 3).min(tokens.len())]
                    .iter()
                    .map(String::as_str)
                    .collect();
                self.words.met(token, &neighbours, surprise);
                self.stats.tokens_learned += 1;
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
        let cells = self.run(&tokens, mode);
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
            return Some(known.id);
        }
        self.next_engram += 1;
        let id = self.next_engram;
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
        Some(id)
    }

    /// A correction: learned twice, as something to get right.
    pub fn correct(&mut self, text: &str, source: &str) -> Option<u64> {
        self.stats.corrections += 1;
        self.learn(text, source);
        self.learn(text, source)
    }

    /// Says what its primed cells lead to after `prompt`, a word at a
    /// time. It does not learn from what it says.
    pub fn say(&mut self, prompt: &str) -> Utterance {
        let tokens = tokenize(prompt);
        self.cortex.reset();
        for token in &tokens {
            self.fire_word("speak", token);
        }
        let chain = self.chain(tokens.last().cloned(), "speak", 0.85, &HashSet::new(), 0.0, LONGEST);
        let said = chain.words;
        // A continuation is judged with the prompt's unfinished sentence.
        let open = tokens
            .iter()
            .rposition(|t| ends_sentence(t) || matches!(t.as_str(), BRAIN | STORY))
            .map_or(0, |at| at + 1);
        let whole: Vec<String> = tokens[open.min(tokens.len())..]
            .iter()
            .chain(&said)
            .cloned()
            .collect();
        let new_sentences = self.new_sentences(&whole);
        let words = said.len().max(1) as f32;
        let utterance = Utterance {
            time_ms: now_ms(),
            prompt: prompt.chars().take(300).collect(),
            text: join(&said),
            novel: !new_sentences.is_empty(),
            new_sentences,
            confidence: chain.confidence / words,
            why: if chain.reasons.is_empty() {
                format!("{} words, each the one its primed cells pointed to", said.len())
            } else {
                chain.reasons.join(" ")
            },
        };
        self.stats.spoken += 1;
        self.stats.novel += u64::from(utterance.novel);
        self.said.push_back(utterance.clone());
        while self.said.len() > SAID {
            self.said.pop_front();
        }
        self.phase = "spoke".into();
        utterance
    }

    /// Finished sentences of three or more words in `tokens` that it was
    /// never taught.
    fn new_sentences(&self, tokens: &[String]) -> Vec<String> {
        let finished = tokens.iter().rposition(|t| ends_sentence(t)).map_or(0, |at| at + 1);
        sentences(&tokens[..finished])
            .into_iter()
            .filter(|s| s.len() >= 3 && !self.sentences.contains(&sentence_hash(s)))
            .map(|s| join(&s))
            .collect()
    }

    /// Lets the primed cells choose word after word, as `process`, from
    /// wherever the cortex is now; `last` is the word just before, to
    /// fall back on when the thread is lost.
    ///
    /// Every word primed at least `reach` as strongly as the best is a
    /// candidate. Among them it decides by how strongly each is primed,
    /// plus `pull` for a word in `towards` (a goal held in mind), plus a
    /// little chance. The first, best-primed word is its intuition; when
    /// the decision goes against it, that is said.
    fn chain(
        &mut self,
        last: Option<String>,
        process: &str,
        reach: f32,
        towards: &HashSet<String>,
        pull: f32,
        longest: usize,
    ) -> Chain {
        let mut said: Vec<String> = Vec::new();
        let mut confidence = 0.0;
        let mut reasons = Vec::new();
        let mut regrouped = 0;
        for _ in 0..longest {
            let mut guesses = self.guesses(8);
            // Lost the thread: the last word alone, out of context, primes
            // whatever has ever followed it.
            if guesses.is_empty()
                && regrouped < 2
                && let Some(last) = said.last().cloned().or(last.clone())
                && let Some(columns) = self.code(&last)
            {
                regrouped += 1;
                self.cortex.reset();
                self.fire(process, &last, &columns, false);
                guesses = self.guesses(8);
                reasons.push(format!(
                    "Lost the thread after \u{201c}{last}\u{201d}; fired it alone, out of context, to find what can follow it."
                ));
            }
            let Some(best) = guesses.first().cloned() else {
                reasons.push(if said.is_empty() {
                    "No branch was primed: it has not learned what follows this.".to_string()
                } else {
                    "No branch was primed any more, so it stopped.".to_string()
                });
                break;
            };
            // A word it just said twice is passed over.
            let near: Vec<(String, f32, u32)> = guesses
                .iter()
                .filter(|g| g.1 >= best.1 * reach)
                .filter(|g| said.iter().rev().take(6).filter(|s| **s == g.0).count() < 2)
                .filter(|g| !matches!(g.0.as_str(), STORY | BRAIN))
                .cloned()
                .collect();
            let mut weighed: Vec<((String, f32, u32), f32)> = Vec::new();
            for g in near {
                let chance = (self.draw() % 1000) as f32 / 1000.0;
                let held = if towards.contains(&g.0) { pull } else { 0.0 };
                let worth = g.1 + held + 0.08 * chance;
                weighed.push((g, worth));
            }
            weighed.sort_by(|a, b| b.1.total_cmp(&a.1));
            let Some((choice, _)) = weighed.first().cloned() else {
                reasons.push("Every likely word would have repeated itself, so it stopped.".into());
                break;
            };
            confidence += choice.1;
            if weighed.len() > 1 {
                self.decided(&choice.0, &best, &weighed);
            }
            let runner = guesses.iter().find(|g| g.0 != choice.0);
            let why = match runner {
                Some(r) => format!(
                    "chose \u{201c}{}\u{201d}: {} of its {ACTIVE} columns primed ({:.0}%); next \u{201c}{}\u{201d} {:.0}%{}",
                    choice.0,
                    choice.2,
                    choice.1 * 100.0,
                    r.0,
                    r.1 * 100.0,
                    if towards.contains(&choice.0) { "; it was also held in mind" } else { "" }
                ),
                None => format!(
                    "chose \u{201c}{}\u{201d}: {} of its {ACTIVE} columns primed, nothing else was",
                    choice.0, choice.2
                ),
            };
            let Some(columns) = self.code(&choice.0) else {
                break;
            };
            let activation = self.fire(process, &choice.0, &columns, false);
            self.steps += 1;
            let step = Step {
                number: self.steps,
                time_ms: now_ms(),
                mode: process.into(),
                token: choice.0.clone(),
                predicted: activation.predicted,
                bursting: activation.bursting,
                surprise: activation.bursting as f32 / ACTIVE as f32,
                spiking: activation.spiking.len() as u32,
                guesses: guesses.clone(),
                chosen: Some(choice.0.clone()),
                why,
                branches: self.cortex.branch_fires(4),
                changes: activation.changes,
            };
            self.record(step);
            if choice.0 == USER {
                break;
            }
            said.push(choice.0.clone());
            if ends_sentence(&choice.0) {
                break;
            }
        }
        Chain {
            words: said,
            confidence,
            reasons,
        }
    }

    /// A choice between near rivals: the cells primed for the chosen word
    /// are lit as deciding, and going against the first hunch is noted.
    fn decided(&mut self, chosen: &str, hunch: &(String, f32, u32), weighed: &[((String, f32, u32), f32)]) {
        let columns: HashSet<u32> = self.words.words.get(chosen).map(|w| w.columns.iter().copied().collect()).unwrap_or_default();
        let mut cells: Vec<u32> = self
            .cortex
            .primed_cells()
            .into_iter()
            .filter(|c| self.cortex.cell(*c).is_some_and(|c| columns.contains(&c.column)))
            .collect();
        cells.sort_unstable();
        self.lived("decide", chosen, cells, 0.0, false);
        if chosen != hunch.0 {
            let options = weighed
                .iter()
                .map(|(g, worth)| (g.0.clone(), *worth))
                .collect();
            self.mind.think(Thought {
                time_ms: now_ms(),
                kind: "decide".into(),
                seed: hunch.0.clone(),
                text: chosen.into(),
                confidence: weighed.first().map_or(0.0, |w| w.0.1),
                path: options,
                why: format!(
                    "Intuition said \u{201c}{}\u{201d} ({:.0}% primed); weighing {} near rivals, it went with \u{201c}{chosen}\u{201d}.",
                    hunch.0,
                    hunch.1 * 100.0,
                    weighed.len()
                ),
                ..Thought::default()
            });
        }
    }

    /// A turn of conversation: it hears what was said, as experience, then
    /// answers.
    pub fn converse(&mut self, text: &str) -> Utterance {
        self.learn(&format!("{USER} {text}"), "user");
        self.say(&format!("{USER} {text} {BRAIN}"))
    }

    /// Recalls the memory a cue best brings back: the engram whose cells
    /// the cue sets spiking overlap most, replayed from its first cells.
    pub fn recall(&mut self, cue: &str) -> Recall {
        let tokens = tokenize(cue);
        self.cortex.reset();
        let mut cue_cells: HashSet<u32> = HashSet::new();
        let mut last: Vec<u32> = Vec::new();
        for token in &tokens {
            if let Some(activation) = self.fire_word("recall", token) {
                cue_cells.extend(&activation.spiking);
                last = activation.spiking;
            }
        }
        // A cell held by many memories says little about which one is
        // meant; one held by a single memory says a lot.
        let held: Vec<HashSet<u32>> = self
            .engrams
            .iter()
            .map(|e| e.cells.iter().flatten().copied().collect())
            .collect();
        let mut holders: HashMap<u32, u32> = HashMap::new();
        for cells in &held {
            for cell in cells.iter().filter(|c| cue_cells.contains(c)) {
                *holders.entry(*cell).or_default() += 1;
            }
        }
        let weight = |cell: &u32| 1.0 / holders.get(cell).copied().unwrap_or(1) as f32;
        let possible: f32 = cue_cells.iter().filter(|c| holders.contains_key(c)).map(weight).sum();
        let mut scored: Vec<(usize, f32)> = held
            .iter()
            .enumerate()
            .map(|(at, cells)| {
                let shared: f32 = cue_cells.intersection(cells).map(weight).sum();
                (at, if possible > 0.0 { shared / possible } else { 0.0 })
            })
            .collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(b.0.cmp(&a.0)));
        let mut recall = Recall {
            time: self.cortex.clock,
            cue: cue.chars().take(200).collect(),
            matched: scored.first().map_or(0.0, |s| s.1),
            runner_up: scored.get(1).map_or(0.0, |s| s.1),
            ..Recall::default()
        };
        let Some(&(at, matched)) = scored.first().filter(|s| s.1 >= RECOGNISED) else {
            recall.why = format!(
                "No memory holds enough of the cells the cue set spiking ({:.0}% at best; {:.0}% needed).",
                recall.matched * 100.0,
                RECOGNISED * 100.0
            );
            self.push_recall(recall.clone());
            return recall;
        };
        let engram = self.engrams[at].clone();
        // Where in the memory the cue's last word was: the step whose
        // stored cells it set spiking most.
        let from = engram
            .cells
            .iter()
            .enumerate()
            .max_by_key(|(step, cells)| {
                (
                    cells.iter().filter(|c| last.contains(c)).count(),
                    usize::MAX - step,
                )
            })
            .map_or(0, |(step, _)| step);
        recall.engram = Some(engram.id);
        self.mind.me.focus_memory = Some(engram.id);
        recall.remembered = engram.text();
        let replay = self.replay_engram(&engram, from, true);
        recall.recalled = replay.0;
        recall.fidelity = replay.1;
        recall.words_right = replay.2;
        recall.words = engram.tokens.len().saturating_sub(from + 1) as u32;
        recall.reignited = engram.cells.get(from).cloned().unwrap_or_default();
        recall.returned = replay.3;
        recall.why = format!(
            "Chose memory {} because it holds {:.0}% of what the cue's cells point to, weighing each cell by how few memories hold it (next best {:.0}%). \
             Re-fired the {} cells it stored for \u{201c}{}\u{201d}; the cortex's own synapses then brought back {:.0}% of the cells it was stored in, and {} of {} words.",
            engram.id,
            matched * 100.0,
            recall.runner_up * 100.0,
            recall.reignited.len(),
            engram.tokens[from],
            recall.fidelity * 100.0,
            recall.words_right,
            recall.words
        );
        let fidelity = recall.fidelity;
        if let Some(e) = self.engrams.get_mut(at) {
            e.recalls += 1;
            e.strength = (e.strength + 0.2).min(5.0);
            e.last_recalled = self.cortex.clock;
            e.fidelity = Some(fidelity);
        }
        self.stats.recalls += 1;
        self.stats.recall_fidelity += 0.2 * (fidelity - self.stats.recall_fidelity);
        self.push_recall(recall.clone());
        self.phase = "recalled a memory".into();
        recall
    }

    fn push_recall(&mut self, recall: Recall) {
        self.recalls.push_back(recall);
        while self.recalls.len() > RECALLS {
            self.recalls.pop_front();
        }
    }

    /// Re-fires an engram's cells at step `from` and lets the cortex carry on,
    /// choosing at each step the word its primed cells point to. Returns
    /// what came back, the share of stored cells that were primed again,
    /// words right, and some of the cells that came back.
    fn replay_engram(
        &mut self,
        engram: &Engram,
        from: usize,
        record: bool,
    ) -> (String, f32, u32, Vec<u32>) {
        let Some(first) = engram.cells.get(from) else {
            return (String::new(), 0.0, 0, Vec::new());
        };
        self.cortex.reset();
        self.reignite("recall", &engram.tokens[from], first);
        let mut recalled = vec![engram.tokens[from].clone()];
        let mut stored = 0;
        let mut back = 0;
        let mut right = 0;
        let mut returned = Vec::new();
        for at in from + 1..engram.tokens.len().min(engram.cells.len()) {
            let primed = self.cortex.primed_cells();
            let held = &engram.cells[at];
            let came: Vec<u32> = held.iter().copied().filter(|c| primed.contains(c)).collect();
            stored += held.len();
            back += came.len();
            if returned.len() < 64 {
                returned.extend(came.iter().take(64 - returned.len()));
            }
            let guesses = self.guesses(3);
            let Some(choice) = guesses.first().map(|g| g.0.clone()) else {
                break;
            };
            right += u32::from(choice == engram.tokens[at]);
            let Some(columns) = self.code(&choice) else {
                break;
            };
            let activation = self.fire("recall", &choice, &columns, false);
            if record {
                self.steps += 1;
                let step = Step {
                    number: self.steps,
                    time_ms: now_ms(),
                    mode: "recall".into(),
                    token: choice.clone(),
                    predicted: activation.predicted,
                    bursting: activation.bursting,
                    surprise: activation.bursting as f32 / ACTIVE as f32,
                    spiking: activation.spiking.len() as u32,
                    guesses,
                    chosen: Some(choice.clone()),
                    why: format!(
                        "{} of the {} cells stored for \u{201c}{}\u{201d} were primed again",
                        came.len(),
                        held.len(),
                        engram.tokens[at]
                    ),
                    branches: self.cortex.branch_fires(4),
                    changes: activation.changes,
                };
                self.record(step);
            }
            recalled.push(choice);
        }
        let fidelity = if stored == 0 { 0.0 } else { back as f32 / stored as f32 };
        (join(&recalled), fidelity, right, returned)
    }

    /// One sleep cycle: replays a memory (learning from it again), and
    /// every few cycles also tests its recall of one, lets words drift,
    /// fades and prunes unused synapses, branches and cells, and lets
    /// unrecalled memories weaken. Returns what it did.
    pub fn sleep(&mut self) -> String {
        self.stats.sleeps += 1;
        if self.engrams.is_empty() {
            self.phase = "asleep: no memories yet".into();
            return self.phase.clone();
        }
        let total: f32 = self.engrams.iter().map(|e| e.strength).sum();
        let mut pick = (self.draw() % 10_000) as f32 / 10_000.0 * total;
        let mut chosen = 0;
        for (at, engram) in self.engrams.iter().enumerate() {
            pick -= engram.strength;
            if pick <= 0.0 {
                chosen = at;
                break;
            }
        }
        let engram = self.engrams[chosen].clone();
        self.run(&engram.tokens, "replay");
        self.stats.replays += 1;
        if let Some(e) = self.engrams.get_mut(chosen) {
            e.replays += 1;
        }
        let mut did = vec![format!(
            "replayed memory {}: \u{201c}{}\u{201d}",
            engram.id,
            engram::join(&engram.tokens.iter().take(12).cloned().collect::<Vec<_>>())
        )];
        let moved = self.organise();
        if moved > 0 {
            did.push(format!("{moved} columns moved nearer the columns they fire with"));
        }
        if self.stats.sleeps % 3 == 0 {
            let cue = join(&engram.tokens.iter().take(3).cloned().collect::<Vec<_>>());
            let recall = self.recall(&cue);
            did.push(format!("tested recall: {:.0}% of cells came back", recall.fidelity * 100.0));
        }
        // Words drift seldom: every drift moves cells that memories rely on.
        let mut drifted = 0;
        if self.stats.sleeps % 30 == 0 {
            drifted = self.words.drift();
            self.stats.drifted += drifted as u64;
        }
        if self.stats.sleeps % 6 == 0 {
            let (synapses, branches, cells) = self.cortex.prune(IDLE, FADING);
            // Unrecalled memories weaken, so that sleep replays them less,
            // but are not forgotten for that: only a full budget forgets.
            for engram in &mut self.engrams {
                engram.strength = (engram.strength * 0.995).max(0.1);
            }
            let forgotten = self.keep_within_budget();
            did.push(format!(
                "{drifted} words drifted; pruned {synapses} synapses, {branches} branches, {cells} cells; forgot {forgotten} memories"
            ));
        }
        self.phase = format!("asleep: {}", did.join("; "));
        self.phase.clone()
    }

    /// What it does when nobody is talking to it, called every few
    /// seconds. Awake, its mind wanders: a thought, an imagining, a hunch,
    /// or a look at itself, while sleep pressure builds. Once drowsy it
    /// sleeps in cycles, faithful replay first and dreams more often as
    /// the night goes on, until the pressure is paid off. Returns what it
    /// did.
    pub fn idle(&mut self) -> String {
        let felt = &mut self.mind.felt;
        if !felt.asleep && felt.pressure >= mind::DROWSY && !self.engrams.is_empty() {
            felt.asleep = true;
            felt.cycle = 0;
        }
        if self.mind.felt.asleep {
            let felt = &mut self.mind.felt;
            felt.cycle += 1;
            let cycle = felt.cycle;
            // Early in the night mostly deep replay, later more dreaming.
            let dreaming = cycle.is_multiple_of(4) || (cycle > 8 && cycle.is_multiple_of(2));
            felt.stage = if dreaming { "dreaming (REM)".into() } else { "deep sleep (replay)".into() };
            felt.pressure = (felt.pressure - 0.08).max(0.0);
            let did = if dreaming {
                let dream = self.dream();
                format!("dreamed: {}", dream.text)
            } else {
                self.sleep()
            };
            let felt = &mut self.mind.felt;
            if felt.pressure < mind::RESTED {
                felt.asleep = false;
                felt.stage = String::new();
                felt.nights += 1;
                self.phase = format!("woke after {cycle} cycles of sleep");
            }
            return did;
        }
        self.mind.felt.pressure += 0.02;
        let roll = self.draw() % 20;
        let thought = match roll {
            0..=9 => self.think(),
            10..=13 => self.imagine(""),
            14..=16 => self.intuit(""),
            _ => self.reflect(),
        };
        format!("{}: {}", thought.kind, thought.text)
    }

    /// One thing leads to another: from what it is most curious about, or
    /// a word of a memory it holds strongly, the primed cells carry on
    /// with nobody asking. Learns nothing.
    pub fn think(&mut self) -> Thought {
        let seed = match self.draw() % 3 {
            0 => self.curiosity(1).into_iter().next().map(|c| c.word),
            1 => self.mind.felt.present.iter().rev().find(|w| w.chars().all(char::is_alphabetic) && w.len() > 2).cloned(),
            _ => None,
        }
        .or_else(|| self.strong_word());
        let Some(seed) = seed else {
            return self.mind.think(Thought {
                time_ms: now_ms(),
                kind: "think".into(),
                why: "Nothing to think about yet: it has learned no words.".into(),
                ..Thought::default()
            });
        };
        self.cortex.reset();
        self.fire_word("think", &seed);
        let chain = self.chain(Some(seed.clone()), "think", 0.7, &HashSet::new(), 0.0, LONGEST);
        let mut whole = vec![seed.clone()];
        whole.extend(chain.words.iter().cloned());
        let new_sentences = self.new_sentences(&whole);
        self.phase = "thinking".into();
        self.mind.think(Thought {
            time_ms: now_ms(),
            kind: "think".into(),
            text: join(&whole),
            novel: !new_sentences.is_empty(),
            confidence: chain.confidence / chain.words.len().max(1) as f32,
            why: format!(
                "Began at \u{201c}{seed}\u{201d} and let its primed cells carry on. {}",
                chain.reasons.join(" ")
            )
            .trim()
            .to_string(),
            seed,
            ..Thought::default()
        })
    }

    /// A word of a memory, drawn by the memories' strength.
    fn strong_word(&mut self) -> Option<String> {
        let at = self.pick_memory()?;
        let words: Vec<String> = self.engrams[at]
            .tokens
            .iter()
            .filter(|t| t.chars().all(char::is_alphabetic) && t.len() > 2)
            .cloned()
            .collect();
        if words.is_empty() {
            return None;
        }
        Some(words[(self.draw() % words.len() as u64) as usize].clone())
    }

    /// A memory, drawn with odds by its strength.
    fn pick_memory(&mut self) -> Option<usize> {
        let total: f32 = self.engrams.iter().map(|e| e.strength).sum();
        if self.engrams.is_empty() || total <= 0.0 {
            return None;
        }
        let mut pick = (self.draw() % 10_000) as f32 / 10_000.0 * total;
        for (at, engram) in self.engrams.iter().enumerate() {
            pick -= engram.strength;
            if pick <= 0.0 {
                return Some(at);
            }
        }
        Some(self.engrams.len() - 1)
    }

    /// Something that never happened, made of things that did. With no
    /// seed, two memories that share a word are spliced: the first is run
    /// up to that word, and from there the second's words are held in mind
    /// and pull the continuation their way. With a seed, the seed is run
    /// and a memory that does not hold it pulls. Learns nothing.
    pub fn imagine(&mut self, seed: &str) -> Thought {
        let mut thought = Thought {
            time_ms: now_ms(),
            kind: "imagine".into(),
            seed: seed.into(),
            ..Thought::default()
        };
        let seeded = tokenize(seed);
        let (context, towards, sources, why) = if seeded.is_empty() {
            let Some((a, b, word)) = self.bridge() else {
                thought.why = "Nothing to imagine with yet: it needs two memories that share a word.".into();
                return self.mind.think(thought);
            };
            let first = &self.engrams[a];
            let second = &self.engrams[b];
            let at = first.tokens.iter().position(|t| *t == word).unwrap_or(0);
            let then = second.tokens.iter().position(|t| *t == word).unwrap_or(0);
            let start = first.tokens[..=at]
                .iter()
                .rposition(|t| ends_sentence(t) || matches!(t.as_str(), STORY | BRAIN | USER))
                .map_or(0, |p| p + 1)
                .min(at);
            (
                first.tokens[start..=at].to_vec(),
                second.tokens[then + 1..].iter().cloned().collect::<HashSet<String>>(),
                vec![first.id, second.id],
                format!(
                    "Began in memory {} up to \u{201c}{word}\u{201d}, which memory {} also holds, and held the rest of memory {} in mind.",
                    first.id, second.id, second.id
                ),
            )
        } else {
            let other = self
                .engrams
                .iter()
                .filter(|e| !seeded.iter().any(|w| e.tokens.contains(w)))
                .map(|e| e.id)
                .collect::<Vec<_>>();
            let pulled = if other.is_empty() {
                None
            } else {
                Some(other[(self.draw() % other.len() as u64) as usize])
            };
            let towards: HashSet<String> = pulled
                .and_then(|id| self.engram(id))
                .map(|e| e.tokens.iter().cloned().collect())
                .unwrap_or_default();
            (
                seeded.clone(),
                towards,
                pulled.into_iter().collect(),
                match pulled {
                    Some(id) => format!("Began from the seed and held memory {id}, which does not hold it, in mind."),
                    None => "Began from the seed; no other memory to hold in mind.".into(),
                },
            )
        };
        self.cortex.reset();
        for token in &context {
            self.fire_word("imagine", token);
        }
        let chain = self.chain(context.last().cloned(), "imagine", 0.5, &towards, 0.25, LONGEST);
        let mut whole = context.clone();
        whole.extend(chain.words.iter().cloned());
        let new_sentences = self.new_sentences(&whole);
        thought.text = join(&whole);
        thought.novel = !new_sentences.is_empty();
        thought.confidence = chain.confidence / chain.words.len().max(1) as f32;
        thought.sources = sources;
        thought.why = format!(
            "{why} {}{}",
            if thought.novel { "It made a sentence it was never taught. " } else { "" },
            chain.reasons.join(" ")
        )
        .trim()
        .to_string();
        self.phase = "imagining".into();
        self.mind.think(thought)
    }

    /// Two memories and a word both hold, the first drawn by strength.
    fn bridge(&mut self) -> Option<(usize, usize, String)> {
        let a = self.pick_memory()?;
        let mut words: Vec<&String> = self.engrams[a]
            .tokens
            .iter()
            .filter(|t| t.chars().all(char::is_alphabetic) && t.len() > 2)
            .collect();
        words.sort();
        words.dedup();
        let mut options = Vec::new();
        for word in words {
            for (b, other) in self.engrams.iter().enumerate() {
                if b != a && other.tokens.contains(word) && other.tokens != self.engrams[a].tokens {
                    options.push((b, word.clone()));
                }
            }
        }
        if options.is_empty() {
            return None;
        }
        let (b, word) = options[(self.draw() % options.len() as u64) as usize].clone();
        Some((a, b, word))
    }

    /// A dream: a fragment of a memory is re-fired, and the cortex runs
    /// free from it, loosely, so that less likely words get their turn.
    /// Where the dream reaches a word another memory holds, it sometimes
    /// jumps into that memory there; when it loses the thread it starts a
    /// new scene. Nothing is learned from a dream.
    pub fn dream(&mut self) -> Thought {
        let mut thought = Thought {
            time_ms: now_ms(),
            kind: "dream".into(),
            ..Thought::default()
        };
        let Some(at) = self.pick_memory() else {
            thought.why = "No memories to dream of.".into();
            return self.mind.think(thought);
        };
        let mut scene = self.engrams[at].clone();
        let mut from = self.fragment(&scene);
        self.cortex.reset();
        let Some(cells) = scene.cells.get(from).cloned() else {
            thought.why = "The memory it began with holds no cells.".into();
            return self.mind.think(thought);
        };
        self.reignite("dream", &scene.tokens[from], &cells);
        thought.seed = scene.tokens[from].clone();
        thought.sources.push(scene.id);
        let mut words = vec![scene.tokens[from].clone()];
        let mut scenes = 1;
        let mut confidence = 0.0;
        for _ in 0..DREAMED {
            let guesses = self.guesses(8);
            if guesses.is_empty() {
                if scenes >= 3 {
                    break;
                }
                let Some(next) = self.pick_memory() else { break };
                scene = self.engrams[next].clone();
                from = self.fragment(&scene);
                let Some(cells) = scene.cells.get(from).cloned() else { break };
                self.reignite("dream", &scene.tokens[from], &cells);
                thought.sources.push(scene.id);
                words.push(scene.tokens[from].clone());
                scenes += 1;
                continue;
            }
            let best = guesses[0].1;
            let mut near: Vec<&(String, f32, u32)> = guesses
                .iter()
                .filter(|g| g.1 >= best * 0.4 && !matches!(g.0.as_str(), STORY | BRAIN | USER))
                .collect();
            if near.is_empty() {
                break;
            }
            let pick = (self.draw() % near.len() as u64) as usize;
            let choice = near.swap_remove(pick).clone();
            confidence += choice.1;
            if self.fire_word("dream", &choice.0).is_none() {
                break;
            }
            words.push(choice.0.clone());
            if ends_sentence(&choice.0) && words.len() > 8 {
                break;
            }
            // A word another memory holds may carry the dream into it.
            if self.draw().is_multiple_of(4) {
                let elsewhere: Vec<(usize, usize)> = self
                    .engrams
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.id != scene.id)
                    .filter_map(|(i, e)| e.tokens.iter().position(|t| *t == choice.0).map(|p| (i, p)))
                    .filter(|(i, p)| self.engrams[*i].cells.get(*p).is_some())
                    .collect();
                if !elsewhere.is_empty() {
                    let (i, p) = elsewhere[(self.draw() % elsewhere.len() as u64) as usize];
                    scene = self.engrams[i].clone();
                    let cells = scene.cells[p].clone();
                    self.reignite("dream", &choice.0, &cells);
                    thought.sources.push(scene.id);
                    scenes += 1;
                }
            }
        }
        let new_sentences = self.new_sentences(&words);
        thought.text = join(&words);
        thought.novel = !new_sentences.is_empty();
        thought.confidence = confidence / words.len().max(1) as f32;
        thought.why = format!(
            "Re-fired memory {} at \u{201c}{}\u{201d} and let the cortex run loosely; it passed through {} scene{} from memories {}.",
            thought.sources[0],
            thought.seed,
            scenes,
            if scenes == 1 { "" } else { "s" },
            thought.sources.iter().map(u64::to_string).collect::<Vec<_>>().join(", ")
        );
        self.phase = "dreaming".into();
        self.mind.think(thought)
    }

    /// A place in a memory to start a dream scene: a word, not a mark.
    fn fragment(&mut self, engram: &Engram) -> usize {
        let words: Vec<usize> = (0..engram.tokens.len().min(engram.cells.len()))
            .filter(|at| !matches!(engram.tokens[*at].as_str(), USER | BRAIN | STORY))
            .collect();
        if words.is_empty() {
            return 0;
        }
        words[(self.draw() % words.len() as u64) as usize]
    }

    /// Reasoning over what it has learned: a search for a chain of steps,
    /// each one a word the cortex primes after the ones before, leading
    /// from `from` to `to`. It keeps the few most likely continuations
    /// open at each step and tries the best primed words from each, as
    /// far as its longest memory. Learns nothing.
    pub fn reason(&mut self, from: &str, to: &str) -> Thought {
        let start = tokenize(from);
        let goal = tokenize(to).into_iter().find(|t| !matches!(t.as_str(), USER | BRAIN | STORY));
        let mut thought = Thought {
            time_ms: now_ms(),
            kind: "reason".into(),
            seed: format!("{from} \u{2192} {to}"),
            ..Thought::default()
        };
        let (Some(goal), false) = (goal, start.is_empty()) else {
            thought.why = "Reasoning needs somewhere to start and something to reach.".into();
            return self.mind.think(thought);
        };
        self.cortex.reset();
        for token in &start {
            self.fire_word("reason", token);
        }
        let depth = self.engrams.iter().map(|e| e.tokens.len()).max().unwrap_or(0).max(8);
        let mut open: Vec<Open> = vec![(self.cortex.state(), Vec::new(), 0.0)];
        let mut explored = 0;
        let mut found = None;
        let mut closest: Option<(Vec<(String, f32)>, f32)> = None;
        'search: for _ in 0..depth {
            let mut next = Vec::new();
            for (state, path, score) in &open {
                self.cortex.restore(state);
                let guesses = self.guesses(BRANCHING * 2);
                if let Some(g) = guesses.iter().find(|g| g.0 == goal) {
                    let mut path = path.clone();
                    path.push((goal.clone(), g.1));
                    found = Some(path);
                    break 'search;
                }
                for g in guesses
                    .iter()
                    .filter(|g| !matches!(g.0.as_str(), USER | BRAIN | STORY))
                    .filter(|g| !path.iter().any(|p| p.0 == g.0) || ends_sentence(&g.0))
                    .take(BRANCHING)
                {
                    self.cortex.restore(state);
                    if self.fire_word("reason", &g.0).is_none() {
                        continue;
                    }
                    explored += 1;
                    let mut path = path.clone();
                    path.push((g.0.clone(), g.1));
                    let score = score + g.1.max(1e-3).ln();
                    let near_goal = self.words.shared(&g.0, &goal) as f32;
                    if closest.as_ref().is_none_or(|c| near_goal > c.1) {
                        closest = Some((path.clone(), near_goal));
                    }
                    next.push((self.cortex.state(), path, score));
                }
            }
            if next.is_empty() {
                break;
            }
            next.sort_by(|a, b| b.2.total_cmp(&a.2));
            next.truncate(BEAM);
            open = next;
        }
        self.phase = "reasoning".into();
        match found {
            Some(path) => {
                let mut words = start.clone();
                words.extend(path.iter().map(|p| p.0.clone()));
                thought.text = join(&words);
                thought.confidence = (path.iter().map(|p| p.1.max(1e-3).ln()).sum::<f32>() / path.len() as f32).exp();
                thought.why = format!(
                    "Found a chain of {} learned step{} from \u{201c}{from}\u{201d} to \u{201c}{goal}\u{201d}, trying {explored} continuations; each link is the share of the next word's columns the cortex primed.",
                    path.len(),
                    if path.len() == 1 { "" } else { "s" }
                );
                thought.path = path;
            }
            None => {
                let closest = closest.map(|c| c.0).unwrap_or_default();
                let mut words = start.clone();
                words.extend(closest.iter().map(|p| p.0.clone()));
                thought.text = join(&words);
                thought.why = format!(
                    "No chain from \u{201c}{from}\u{201d} reached \u{201c}{goal}\u{201d} within {depth} steps, after {explored} continuations. Shown: the path that came closest, by columns shared with it."
                );
                thought.path = closest;
            }
        }
        self.mind.think(thought)
    }

    /// Intuition: the cue is fired once and the best-primed word taken at
    /// once, without chaining or weighing, together with how often a
    /// guess that sure has come true before.
    pub fn intuit(&mut self, cue: &str) -> Thought {
        let cue = if cue.trim().is_empty() {
            self.curiosity(1).into_iter().next().map(|c| c.word).or_else(|| self.strong_word()).unwrap_or_default()
        } else {
            cue.to_string()
        };
        let mut thought = Thought {
            time_ms: now_ms(),
            kind: "intuit".into(),
            seed: cue.clone(),
            ..Thought::default()
        };
        self.cortex.reset();
        for token in tokenize(&cue) {
            self.fire_word("intuit", &token);
        }
        let guesses = self.guesses(5);
        match guesses.first() {
            Some(hunch) => {
                thought.text = hunch.0.clone();
                thought.confidence = hunch.1;
                thought.path = guesses.iter().map(|g| (g.0.clone(), g.1)).collect();
                thought.why = match self.mind.me.expect(hunch.1) {
                    Some(right) => format!(
                        "First thing primed after \u{201c}{cue}\u{201d}: {:.0}% of its columns. Its guesses this sure have come true {:.0}% of the time.",
                        hunch.1 * 100.0,
                        right * 100.0
                    ),
                    None => format!(
                        "First thing primed after \u{201c}{cue}\u{201d}: {:.0}% of its columns. It has too little record yet to say how far to trust that.",
                        hunch.1 * 100.0
                    ),
                };
            }
            None => thought.why = format!("Nothing is primed after \u{201c}{cue}\u{201d}: no hunch."),
        }
        self.phase = "following a hunch".into();
        self.mind.think(thought)
    }

    /// Looks at itself: brings what it was last attending to back to mind
    /// and says, from its own measurements, what state it is in.
    pub fn reflect(&mut self) -> Thought {
        if self.cortex.active().is_empty()
            && let Some(word) = self.mind.felt.present.back().cloned()
        {
            self.fire_word("aware", &word);
        } else {
            let cells = self.cortex.active().to_vec();
            let word = self.mind.felt.present.back().cloned().unwrap_or_default();
            self.lived("aware", &word, cells, 0.0, false);
        }
        let report = self.report();
        self.mind.me.report = report.clone();
        self.phase = "reflecting".into();
        self.mind.think(Thought {
            time_ms: now_ms(),
            kind: "reflect".into(),
            text: report,
            confidence: self.mind.me.confidence,
            why: "Read from its own measurements: felt time, episodes, sureness and how far it has held, sleep pressure, curiosity.".into(),
            ..Thought::default()
        })
    }

    /// What it would say of itself now, every clause a measurement.
    pub fn report(&self) -> String {
        let felt = &self.mind.felt;
        let me = &self.mind.me;
        let mut parts = Vec::new();
        parts.push(if felt.asleep {
            format!("I am asleep, {}, cycle {}.", felt.stage, felt.cycle)
        } else if me.doing.is_empty() {
            "I am awake.".to_string()
        } else {
            format!("I am awake; last I was {}.", describe(&me.doing))
        });
        if felt.steps > 0 {
            parts.push(format!(
                "{:.0} moments have felt to pass over {} steps; lately a step feels like {:.2} moments, so time is {}.",
                felt.moments,
                felt.steps,
                felt.rate,
                if felt.rate > 1.4 { "stretching" } else if felt.rate < 0.6 { "rushing by" } else { "passing evenly" }
            ));
            parts.push(format!(
                "This is episode {}, which began at \u{201c}{}\u{201d}, {:.1} moments ago.",
                felt.now.id, felt.now.opening, felt.now.felt
            ));
        }
        if !felt.present.is_empty() {
            parts.push(format!(
                "In the present: \u{201c}{}\u{201d}.",
                join(&felt.present.iter().cloned().collect::<Vec<_>>())
            ));
        }
        match self.mind.me.expect(me.confidence) {
            Some(right) => parts.push(format!(
                "My first guesses lately are {:.0}% sure, and guesses that sure have come true {:.0}% of the time.",
                me.confidence * 100.0,
                right * 100.0
            )),
            None => parts.push("I have too little record yet to know how far to trust my guesses.".into()),
        }
        if let Some(insight) = self.mind.me.insight() {
            parts.push(if insight > 0.15 {
                format!("When I am surer I am right more often ({:.0} points more), so I can tell what I know.", insight * 100.0)
            } else {
                "Being surer hardly makes me more often right: I cannot yet tell what I know from what I do not.".into()
            });
        }
        let change = me.right_now - me.right_before;
        parts.push(
            if change > 0.05 {
                "I am predicting better than I was."
            } else if change < -0.05 {
                "I am predicting worse than I was: this is new to me."
            } else {
                "I predict about as well as I have been."
            }
            .into(),
        );
        if let Some(curious) = self.curiosity(1).first() {
            parts.push(format!("I am most curious about \u{201c}{}\u{201d}.", curious.word));
        }
        parts.push(format!(
            "Sleep pressure {:.2}; I fall asleep at {:.1}.",
            felt.pressure,
            mind::DROWSY
        ));
        parts.join(" ")
    }

    /// Every cell and synapse and each memory's cells, for drawing.
    pub fn wiring(&mut self) -> Wiring {
        self.mind.map.fit(self.cortex.area());
        let synapses = self.cortex.wiring();
        Wiring {
            clock: self.cortex.clock,
            area: self.cortex.area(),
            slot_of: self.mind.map.slot_of.clone(),
            cells: self.cortex.cell_columns(),
            synapse_count: synapses.len(),
            synapses: mind::pack(synapses),
            engrams: self
                .engrams
                .iter()
                .map(|e| {
                    let mut cells: Vec<u32> = e.cells.iter().flatten().copied().collect();
                    cells.sort_unstable();
                    cells.dedup();
                    (e.id, cells)
                })
                .collect(),
        }
    }

    /// Lets the map's columns trade places towards their words.
    fn organise(&mut self) -> usize {
        let area = self.cortex.area();
        if self.slots.len() != area {
            self.slots = map::slots(area);
        }
        let mut random = self.draw();
        let slots = std::mem::take(&mut self.slots);
        let made = self.mind.map.organise(&self.words, &slots, ORGANISE, &mut random);
        self.slots = slots;
        made
    }

    /// Words it is most surprised by and still learning about.
    pub fn curiosity(&self, most: usize) -> Vec<Curiosity> {
        let mut wanted: Vec<Curiosity> = self
            .words
            .words
            .iter()
            .filter(|(t, w)| {
                w.seen >= 2
                    && t.chars().count() > 2
                    && t.chars().all(char::is_alphabetic)
            })
            .map(|(t, w)| Curiosity {
                word: t.clone(),
                surprise: w.surprise,
                progress: w.surprise_before - w.surprise,
                seen: w.seen,
            })
            .collect();
        let want = |c: &Curiosity| c.surprise * (1.0 + 2.0 * c.progress.max(0.0));
        wanted.sort_by(|a, b| want(b).total_cmp(&want(a)).then(a.word.cmp(&b.word)));
        wanted.truncate(most);
        wanted
    }

    pub fn snapshot(&self) -> NeuralSnapshot {
        let (synapses, connected) = self.cortex.synapse_counts();
        let columns = self
            .cortex
            .columns_view()
            .into_iter()
            .map(|(_, cells, branches, _)| (cells, branches))
            .collect();
        let mut primed: Vec<u32> = self.cortex.primed_columns().into_keys().collect();
        primed.sort_unstable();
        let engrams = self
            .engrams
            .iter()
            .rev()
            .enumerate()
            .map(|(at, e)| {
                // Every memory is listed; the columns of the newest are
                // enough for the view, which takes every memory's cells
                // from the wiring.
                let mut columns: Vec<u32> = e
                    .cells
                    .iter()
                    .flatten()
                    .filter(|_| at < ENGRAM_COLUMNS)
                    .filter_map(|c| self.cortex.cell(*c).map(|c| c.column))
                    .collect();
                columns.sort_unstable();
                columns.dedup();
                EngramView {
                    id: e.id,
                    text: e.text().chars().take(240).collect(),
                    source: e.source.clone(),
                    strength: e.strength,
                    recalls: e.recalls,
                    replays: e.replays,
                    cells: e.cell_count() as u32,
                    columns,
                    fidelity: e.fidelity,
                    created: e.created,
                }
            })
            .collect();
        NeuralSnapshot {
            stats: self.stats.clone(),
            totals: self.cortex.totals.clone(),
            clock: self.cortex.clock,
            vocabulary: self.words.len(),
            cells: self.cortex.cells_alive(),
            branches: self.cortex.segments_alive(),
            synapses,
            connected,
            permanences: self.cortex.permanences(),
            columns,
            recent: self.recent.iter().cloned().collect(),
            primed,
            engrams,
            engram_count: self.engrams.len(),
            trace: self.trace.iter().cloned().collect(),
            recalls: self.recalls.iter().cloned().collect(),
            said: self.said.iter().cloned().collect(),
            curious: self.curiosity(10),
            phase: self.phase.clone(),
            area: self.cortex.area(),
            footprint: self.footprint(),
            budget: self.budget(),
            mind: self.mind_view(),
        }
    }

    /// The mind for the view, with the last few seconds of activity.
    fn mind_view(&self) -> MindView {
        let since = now_ms().saturating_sub(ACTIVITY_WINDOW_MS);
        let mut slot_of = self.mind.map.slot_of.clone();
        while slot_of.len() < self.cortex.area() {
            slot_of.push(slot_of.len() as u32);
        }
        MindView {
            felt: self.mind.felt.clone(),
            me: SelfModel {
                report: if self.mind.me.report.is_empty() { self.report() } else { self.mind.me.report.clone() },
                ..self.mind.me.clone()
            },
            expected: self.mind.me.expect(self.mind.me.confidence),
            insight: self.mind.me.insight(),
            thoughts: self.mind.thoughts.iter().cloned().collect(),
            activity: self.activity.iter().filter(|a| a.time_ms >= since).cloned().collect(),
            slot_of,
            swaps: self.mind.map.swaps,
        }
    }

    /// A short account for the teacher: how far it has come, what it
    /// said lately, and what it is curious about.
    pub fn progress(&self) -> serde_json::Value {
        serde_json::json!({
            "vocabulary": self.words.len(),
            "memories": self.engrams.len(),
            "lessons": self.stats.lessons,
            "next_word_right_recently": (self.stats.recent_right * 100.0).round() / 100.0,
            "commonest_word_baseline_recently": (self.stats.recent_baseline * 100.0).round() / 100.0,
            "recall_fidelity": (self.stats.recall_fidelity * 100.0).round() / 100.0,
            "curious_about": self.curiosity(6).into_iter().map(|c| c.word).collect::<Vec<_>>(),
            "recently_said": self.said.iter().rev().take(3).map(|u| &u.text).collect::<Vec<_>>(),
        })
    }

    pub fn load(file: &Path) -> std::io::Result<Self> {
        let bytes = fs::read(file)?;
        let mut brain: Brain = postcard::from_bytes(&bytes).map_err(std::io::Error::other)?;
        if !brain.is_sound() {
            return Err(std::io::Error::other("the saved brain is not in working order"));
        }
        // A brain saved before it had a mind starts one now.
        if let Ok(text) = fs::read_to_string(mind_file(file))
            && let Ok(mind) = serde_json::from_str(&text)
        {
            brain.mind = mind;
        }
        brain.mind.map.fit(brain.cortex.area());
        // A map never organised gets a long first pass, so that related
        // columns sit together from the first look rather than after
        // many nights.
        if brain.mind.map.swaps == 0 {
            for _ in 0..10 {
                brain.organise();
            }
        }
        Ok(brain)
    }

    /// Written beside the file and moved over it.
    pub fn save(&self, file: &Path) -> std::io::Result<()> {
        let folder = file
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(folder)?;
        let bytes = postcard::to_stdvec(self).map_err(std::io::Error::other)?;
        let mut partial = tempfile::NamedTempFile::new_in(folder)?;
        partial.write_all(&bytes)?;
        partial.as_file().sync_all()?;
        partial.persist(file).map_err(|e| e.error)?;
        let mind = serde_json::to_vec(&self.mind).map_err(std::io::Error::other)?;
        let mut partial = tempfile::NamedTempFile::new_in(folder)?;
        partial.write_all(&mind)?;
        partial.as_file().sync_all()?;
        partial.persist(mind_file(file)).map_err(|e| e.error)?;
        Ok(())
    }

    /// Engram ids and their cells, for tests and inspection.
    pub fn engram(&self, id: u64) -> Option<&Engram> {
        self.engrams.iter().find(|e| e.id == id)
    }
}

/// Where the mind is kept, beside the brain's file.
pub fn mind_file(file: &Path) -> std::path::PathBuf {
    file.with_extension("mind.json")
}

/// A process, as said of oneself.
fn describe(process: &str) -> &'static str {
    match process {
        "learn" => "being taught",
        "hear" => "listening",
        "speak" | "decide" => "speaking",
        "recall" => "remembering",
        "replay" => "replaying a memory",
        "dream" => "dreaming",
        "think" => "thinking",
        "imagine" => "imagining",
        "reason" => "reasoning",
        "intuit" => "following a hunch",
        "aware" => "looking at myself",
        _ => "busy",
    }
}

/// Which columns each word fires, for the view's legend.
pub fn columns_of(brain: &Brain, word: &str) -> Vec<u32> {
    brain
        .words
        .words
        .get(word)
        .map(|w| w.columns.clone())
        .unwrap_or_default()
}

/// Counts per column in a list, for drawing heat.
pub fn heat(recent: &[Vec<u32>]) -> HashMap<u32, f32> {
    let mut heat = HashMap::new();
    for (age, columns) in recent.iter().rev().enumerate() {
        let weight = 0.75_f32.powi(age as i32);
        for column in columns {
            let value = heat.entry(*column).or_insert(0.0);
            *value = f32::max(*value, weight);
        }
    }
    heat
}

#[cfg(test)]
mod tests;
