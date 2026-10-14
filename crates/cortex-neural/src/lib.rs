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
//! [`Brain`] ties them together: it learns text, speaks by letting its
//! primed cells choose each next word, recalls, and sleeps (replays its
//! memories, lets words drift, prunes). Every step it takes is recorded
//! with the numbers behind it, for the metrics view.
//!
//! What this is, plainly: a sequence-learning network that is surprised by
//! what it did not predict and learns from it. It writes by chaining what
//! its branches predict, and can put together sentences it was not taught
//! where words share columns. It is not a language model and nothing here
//! shows awareness.

pub mod cortex;
pub mod engram;
pub mod words;

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

pub use cortex::{BranchFire, Changes, Cortex, Totals};
pub use engram::{Engram, Recall, join};
use words::{ACTIVE, BRAIN, STORY, USER, Words, ends_sentence, tokenize};

/// Steps kept for the metrics view.
const TRACE: usize = 240;
const RECALLS: usize = 24;
const SAID: usize = 24;

/// Words it says at most in one go.
const LONGEST: usize = 40;

/// Steps unused before a branch fades during sleep, and how much.
const IDLE: u64 = 30_000;
const FADING: f32 = 0.97;

/// Cue cells an engram must share for recall to choose it.
const RECOGNISED: f32 = 0.05;

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
    pub columns: Vec<u16>,
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
    pub columns: Vec<(u8, u16)>,

    /// The columns that fired in the latest steps, oldest first, and the
    /// columns primed now.
    pub recent: Vec<Vec<u16>>,
    pub primed: Vec<u16>,

    pub engrams: Vec<EngramView>,
    pub engram_count: usize,
    pub trace: Vec<Step>,
    pub recalls: Vec<Recall>,
    pub said: Vec<Utterance>,
    pub curious: Vec<Curiosity>,
    pub phase: String,
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
    recent: VecDeque<Vec<u16>>,
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
        self.cortex.is_sound() && self.engrams.len() <= engram::ENGRAMS
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

    fn remember_columns(&mut self, columns: &[u16]) {
        self.recent.push_back(columns.to_vec());
        while self.recent.len() > 16 {
            self.recent.pop_front();
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
            let Some(columns) = self.words.columns(token) else {
                continue;
            };
            let guesses = if at > 0 { self.guesses(5) } else { Vec::new() };
            if scored && at > 0 {
                let right = guesses.first().is_some_and(|g| &g.0 == token);
                let baseline = commonest.as_ref() == Some(token);
                self.stats.scored += 1;
                self.stats.right += u64::from(right);
                self.stats.baseline_right += u64::from(baseline);
                self.stats.recent_right += 0.02 * (f32::from(u8::from(right)) - self.stats.recent_right);
                self.stats.recent_baseline +=
                    0.02 * (f32::from(u8::from(baseline)) - self.stats.recent_baseline);
            }
            let activation = self.cortex.step(&columns, learn);
            let surprise = activation.bursting as f32 / ACTIVE as f32;
            if scored {
                let neighbours: Vec<&str> = tokens[at.saturating_sub(2)..(at + 3).min(tokens.len())]
                    .iter()
                    .map(String::as_str)
                    .collect();
                self.words.met(token, &neighbours, surprise);
                self.stats.tokens_learned += 1;
            }
            self.remember_columns(&columns);
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
        let mut tokens = tokenize(text);
        tokens.truncate(engram::STEPS);
        if tokens.len() < 2 {
            return None;
        }
        let mode = if source == "user" { "hear" } else { "learn" };
        let cells = self.run(&tokens, mode);
        for sentence in sentences(&tokens) {
            self.sentences.insert(sentence_hash(&sentence));
        }
        self.stats.lessons += 1;
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
        if self.engrams.len() > engram::ENGRAMS
            && let Some(weakest) = self
                .engrams
                .iter()
                .enumerate()
                .min_by(|a, b| a.1.strength.total_cmp(&b.1.strength))
                .map(|(at, _)| at)
        {
            self.engrams.remove(weakest);
            self.stats.engrams_forgotten += 1;
        }
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
        let mut tokens = tokenize(prompt);
        tokens.truncate(engram::STEPS);
        self.cortex.reset();
        for token in &tokens {
            if let Some(columns) = self.words.columns(token) {
                self.cortex.step(&columns, false);
                self.remember_columns(&columns);
            }
        }
        let mut said: Vec<String> = Vec::new();
        let mut confidence = 0.0;
        let mut reasons = Vec::new();
        let mut regrouped = 0;
        for _ in 0..LONGEST {
            let mut guesses = self.guesses(8);
            // Lost the thread: the last word alone, out of context, primes
            // whatever has ever followed it.
            if guesses.is_empty()
                && regrouped < 2
                && let Some(last) = said.last().or(tokens.last()).cloned()
                && let Some(columns) = self.words.columns(&last)
            {
                regrouped += 1;
                self.cortex.reset();
                self.cortex.step(&columns, false);
                guesses = self.guesses(8);
                reasons.push(format!(
                    "Lost the thread after \u{201c}{last}\u{201d}; fired it alone, out of context, to find what can follow it."
                ));
            }
            let Some(best) = guesses.first().cloned() else {
                reasons.push(if said.is_empty() {
                    "No branch was primed after the prompt: it has not learned what follows this.".to_string()
                } else {
                    "No branch was primed any more, so it stopped.".to_string()
                });
                break;
            };
            // Among guesses near the best, one is drawn, so that it does not
            // always say the same; a word it just said twice is passed over.
            let near: Vec<(String, f32, u32)> = guesses
                .iter()
                .filter(|g| g.1 >= best.1 * 0.85)
                .filter(|g| said.iter().rev().take(6).filter(|s| **s == g.0).count() < 2)
                .filter(|g| !matches!(g.0.as_str(), STORY | BRAIN))
                .cloned()
                .collect();
            let Some(choice) = (if near.is_empty() {
                None
            } else {
                let at = (self.draw() % near.len() as u64) as usize;
                Some(near[at].clone())
            }) else {
                reasons.push("Every likely word would have repeated itself, so it stopped.".into());
                break;
            };
            confidence += choice.1;
            let runner = guesses.iter().find(|g| g.0 != choice.0);
            let why = match runner {
                Some(r) => format!(
                    "chose \u{201c}{}\u{201d}: {} of its {ACTIVE} columns primed ({:.0}%); next \u{201c}{}\u{201d} {:.0}%",
                    choice.0,
                    choice.2,
                    choice.1 * 100.0,
                    r.0,
                    r.1 * 100.0
                ),
                None => format!(
                    "chose \u{201c}{}\u{201d}: {} of its {ACTIVE} columns primed, nothing else was",
                    choice.0, choice.2
                ),
            };
            let Some(columns) = self.words.columns(&choice.0) else {
                break;
            };
            let activation = self.cortex.step(&columns, false);
            self.remember_columns(&columns);
            self.steps += 1;
            let step = Step {
                number: self.steps,
                time_ms: now_ms(),
                mode: "speak".into(),
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
        // Only finished sentences are judged.
        let finished = whole.iter().rposition(|t| ends_sentence(t)).map_or(0, |at| at + 1);
        let new_sentences: Vec<String> = sentences(&whole[..finished])
            .into_iter()
            .filter(|s| s.len() >= 3 && !self.sentences.contains(&sentence_hash(s)))
            .map(|s| join(&s))
            .collect();
        let words = said.len().max(1) as f32;
        let utterance = Utterance {
            time_ms: now_ms(),
            prompt: prompt.chars().take(300).collect(),
            text: join(&said),
            novel: !new_sentences.is_empty(),
            new_sentences,
            confidence: confidence / words,
            why: if reasons.is_empty() {
                format!("{} words, each the one its primed cells pointed to", said.len())
            } else {
                reasons.join(" ")
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
            if let Some(columns) = self.words.columns(token) {
                let activation = self.cortex.step(&columns, false);
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
        self.cortex.reactivate(first);
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
            let Some(columns) = self.words.columns(&choice) else {
                break;
            };
            let activation = self.cortex.step(&columns, false);
            self.remember_columns(&columns);
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
            for engram in &mut self.engrams {
                engram.strength *= 0.995;
            }
            let before = self.engrams.len();
            self.engrams.retain(|e| e.strength >= 0.05);
            let forgotten = before - self.engrams.len();
            self.stats.engrams_forgotten += forgotten as u64;
            did.push(format!(
                "{drifted} words drifted; pruned {synapses} synapses, {branches} branches, {cells} cells; forgot {forgotten} memories"
            ));
        }
        self.phase = format!("asleep: {}", did.join("; "));
        self.phase.clone()
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
            .map(|(_, cells, branches, _)| (cells.min(255) as u8, branches.min(u32::from(u16::MAX)) as u16))
            .collect();
        let mut primed: Vec<u16> = self.cortex.primed_columns().into_keys().collect();
        primed.sort_unstable();
        let engrams = self
            .engrams
            .iter()
            .rev()
            .take(80)
            .map(|e| {
                let mut columns: Vec<u16> = e
                    .cells
                    .iter()
                    .flatten()
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
            trace: self.trace.iter().rev().take(80).rev().cloned().collect(),
            recalls: self.recalls.iter().cloned().collect(),
            said: self.said.iter().cloned().collect(),
            curious: self.curiosity(10),
            phase: self.phase.clone(),
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
        let brain: Brain = postcard::from_bytes(&bytes).map_err(std::io::Error::other)?;
        if !brain.is_sound() {
            return Err(std::io::Error::other("the saved brain is not in working order"));
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
        Ok(())
    }

    /// Engram ids and their cells, for tests and inspection.
    pub fn engram(&self, id: u64) -> Option<&Engram> {
        self.engrams.iter().find(|e| e.id == id)
    }
}

/// Which columns each word fires, for the view's legend.
pub fn columns_of(brain: &Brain, word: &str) -> Vec<u16> {
    brain
        .words
        .words
        .get(word)
        .map(|w| w.columns.clone())
        .unwrap_or_default()
}

/// Counts per column in a list, for drawing heat.
pub fn heat(recent: &[Vec<u16>]) -> HashMap<u16, f32> {
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
