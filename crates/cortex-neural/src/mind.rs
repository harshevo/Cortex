//! What the brain keeps about its own activity: how much time felt like it
//! passed, where one event ended and the next began, how sure it tends to
//! be and how often that sureness is right, and the thoughts, dreams,
//! imaginings and chains of reasoning it has had.
//!
//! There is no clock in a brain. What a person experiences as time is put
//! together from change: the present is a window of a few seconds over
//! which events are bound into one "now"; experience is cut into events
//! where prediction fails; an interval full of new things feels long
//! afterwards and routine shrinks; replay in sleep runs many times faster
//! than the experience it replays; and sleep pressure builds while awake
//! and is paid off in sleep. [`Felt`] measures those from the cortex's own
//! surprise. The steps it counts are the cortex's; wall-clock time is kept
//! beside it only for comparison.
//!
//! [`SelfModel`] is the brain's measurement of itself: what it attends to
//! now, how surprised and how sure it has been, and how often its
//! sureness turned out right. The last is what lets it know what it does
//! not know. It is a model of its own state built from its own numbers,
//! the functional part of self-awareness; nothing here shows experience.

use std::collections::VecDeque;

use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::map::Map;

/// Everything the cortex does is one of these. The view gives each its
/// colour.
pub const PROCESSES: [&str; 12] = [
    "learn", "hear", "speak", "recall", "replay", "dream", "think", "imagine", "reason", "intuit",
    "decide", "aware",
];

/// Episodes and thoughts kept on file.
const EPISODES: usize = 200;
const THOUGHTS: usize = 80;

/// Words in the present moment.
const PRESENT: usize = 7;

/// Sleep pressure at which it falls asleep, and below which it wakes.
pub const DROWSY: f32 = 1.0;
pub const RESTED: f32 = 0.15;

/// One step of the cortex, by whichever process ran it: the cells that
/// spiked and those it primed next. The view lights these up.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Activity {
    pub number: u64,
    pub time_ms: u64,
    pub process: String,
    pub word: String,
    #[serde(with = "packed")]
    pub cells: Vec<u32>,
    #[serde(with = "packed")]
    pub primed: Vec<u32>,
    /// Synapses changed: the spiking winners learned.
    pub learned: bool,
}

/// A stretch of experience between two surprises.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Episode {
    pub id: u64,
    pub began_ms: u64,
    pub ended_ms: u64,
    pub steps: u64,
    /// Time as it felt, in moments.
    pub felt: f64,
    pub surprise: f32,
    pub process: String,
    /// Its first words, and the surprise that opened it.
    pub opening: String,
    pub boundary: f32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Felt {
    /// Time as it felt, in moments: a step counts for more the more it
    /// surprised, and for much less in replay and dreams.
    pub moments: f64,
    pub steps: u64,
    pub born_ms: u64,
    /// Running mean and spread of surprise, against which a boundary is
    /// judged.
    pub surprise: f32,
    pub spread: f32,
    /// Moments per step lately: above 1 time is dilating, below it is
    /// compressing.
    pub rate: f32,
    pub now: Episode,
    pub episodes: VecDeque<Episode>,
    /// The present: the last few words, bound into one moment.
    pub present: VecDeque<String>,
    pub pressure: f32,
    pub asleep: bool,
    /// Sleep stage now, and cycles into this sleep.
    pub stage: String,
    pub cycle: u32,
    pub nights: u64,
}

/// How a step counts towards felt time.
fn weight(process: &str, surprise: f32) -> f64 {
    let novelty = 1.0 + 2.0 * f64::from(surprise);
    match process {
        "replay" | "dream" => 0.08 * novelty,
        "recall" => 0.5 * novelty,
        _ => novelty,
    }
}

impl Felt {
    /// One step lived. Returns whether it opened a new episode.
    pub fn tick(&mut self, process: &str, word: &str, surprise: f32, learned: bool, now_ms: u64) -> bool {
        if self.born_ms == 0 {
            self.born_ms = now_ms;
        }
        self.steps += 1;
        let felt = weight(process, surprise);
        self.moments += felt;
        self.rate += 0.05 * (felt as f32 - self.rate);
        self.present.push_back(word.to_string());
        while self.present.len() > PRESENT {
            self.present.pop_front();
        }
        // A boundary: a surprise well beyond the usual, or a change of what
        // the brain is doing.
        let unusual = surprise > self.surprise + (2.0 * self.spread).max(0.15);
        let switched = !self.now.process.is_empty() && kind(&self.now.process) != kind(process);
        let boundary = self.now.steps >= 3 && (unusual || switched);
        if boundary || self.now.steps == 0 && self.now.id == 0 {
            if self.now.steps > 0 {
                let mut ended = std::mem::take(&mut self.now);
                ended.ended_ms = now_ms;
                self.episodes.push_back(ended);
                while self.episodes.len() > EPISODES {
                    self.episodes.pop_front();
                }
            }
            let id = self.episodes.back().map_or(0, |e| e.id) + 1;
            self.now = Episode {
                id,
                began_ms: now_ms,
                process: process.into(),
                boundary: surprise,
                ..Episode::default()
            };
        }
        let episode = &mut self.now;
        episode.steps += 1;
        episode.felt += felt;
        episode.ended_ms = now_ms;
        episode.surprise += (surprise - episode.surprise) / episode.steps as f32;
        if episode.opening.split(' ').count() < 6 {
            if !episode.opening.is_empty() {
                episode.opening.push(' ');
            }
            episode.opening.push_str(word);
        }
        self.spread += 0.05 * ((surprise - self.surprise).abs() - self.spread);
        self.surprise += 0.05 * (surprise - self.surprise);
        // Learning tires it; using what it knows hardly does.
        if !self.asleep && learned {
            self.pressure += 0.004;
        }
        boundary
    }
}

/// Waking processes are one kind of experience, sleeping ones another.
fn kind(process: &str) -> &'static str {
    match process {
        "replay" | "dream" => "sleep",
        "learn" | "hear" => "perceive",
        "think" | "imagine" | "reason" | "intuit" => "mind",
        _ => "act",
    }
}

/// Of its top guesses made this sure (in tenths), how many were right.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SelfModel {
    pub sureness: [(u32, u32); 10],
    /// Fast averages: surprise, sureness of the top guess, and whether the
    /// top guess was right, now and a while ago.
    pub surprise: f32,
    pub confidence: f32,
    pub right_now: f32,
    pub right_before: f32,
    /// What it is attending to: the words and memory in the present.
    pub focus: Vec<String>,
    pub focus_memory: Option<u64>,
    pub doing: String,
    /// What it says of itself, from the numbers above.
    pub report: String,
}

impl SelfModel {
    /// A top guess and whether it came.
    pub fn guessed(&mut self, sureness: f32, right: bool) {
        let bin = ((sureness * 10.0) as usize).min(9);
        self.sureness[bin].0 += 1;
        self.sureness[bin].1 += u32::from(right);
        self.confidence += 0.05 * (sureness - self.confidence);
        let hit = f32::from(u8::from(right));
        self.right_now += 0.05 * (hit - self.right_now);
        self.right_before += 0.005 * (hit - self.right_before);
    }

    /// How often a guess this sure has come true, once there is enough
    /// record to say; from the neighbouring tenths when this one is thin.
    pub fn expect(&self, sureness: f32) -> Option<f32> {
        let bin = ((sureness * 10.0) as usize).min(9);
        for reach in 0..10 {
            let (mut tries, mut right) = (0, 0);
            for at in bin.saturating_sub(reach)..=(bin + reach).min(9) {
                tries += self.sureness[at].0;
                right += self.sureness[at].1;
            }
            if tries >= 8 {
                return Some(right as f32 / tries as f32);
            }
        }
        None
    }

    /// How well its sureness tells right from wrong: the hit rate of its
    /// surer half of guesses minus that of its less sure half. Near zero,
    /// it does not know when it knows.
    pub fn insight(&self) -> Option<f32> {
        let total: u32 = self.sureness.iter().map(|b| b.0).sum();
        if total < 20 {
            return None;
        }
        let mut seen = 0;
        let (mut low, mut high) = ((0, 0), (0, 0));
        for (tries, right) in self.sureness {
            let half = if seen < total / 2 { &mut low } else { &mut high };
            half.0 += tries;
            half.1 += right;
            seen += tries;
        }
        let rate = |h: (u32, u32)| if h.0 == 0 { 0.0 } else { h.1 as f32 / h.0 as f32 };
        (low.0 > 0 && high.0 > 0).then(|| rate(high) - rate(low))
    }
}

/// A thought it had, of whatever kind, with how it got there.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Thought {
    pub number: u64,
    pub time_ms: u64,
    /// think, imagine, dream, reason, intuit, decide, reflect.
    pub kind: String,
    pub seed: String,
    pub text: String,
    /// It holds a sentence it was never taught.
    pub novel: bool,
    pub confidence: f32,
    /// Each link and how strongly it was primed, for reasoning; the
    /// memories drawn on, for dreams and imagination.
    pub path: Vec<(String, f32)>,
    pub sources: Vec<u64>,
    pub why: String,
}

/// What is kept on file beside the cortex.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mind {
    pub felt: Felt,
    pub me: SelfModel,
    pub thoughts: VecDeque<Thought>,
    pub map: Map,
    pub numbered: u64,
}

impl Mind {
    pub fn think(&mut self, mut thought: Thought) -> Thought {
        self.numbered += 1;
        thought.number = self.numbered;
        self.thoughts.push_back(thought.clone());
        while self.thoughts.len() > THOUGHTS {
            self.thoughts.pop_front();
        }
        thought
    }
}

/// The mind as the view sees it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MindView {
    pub felt: Felt,
    pub me: SelfModel,
    /// How often a guess as sure as its recent ones comes true, and how
    /// well sureness separates right from wrong.
    pub expected: Option<f32>,
    pub insight: Option<f32>,
    pub thoughts: Vec<Thought>,
    /// Steps of the last few seconds, by every process, oldest first.
    pub activity: Vec<Activity>,
    /// The slot of each column on the drawn cortex.
    pub slot_of: Vec<u32>,
    pub swaps: u64,
}

/// Every cell and synapse, for drawing the whole cortex.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Wiring {
    pub clock: u64,
    pub area: usize,
    pub slot_of: Vec<u32>,
    /// The column of each cell id; `u32::MAX` for a dead one.
    pub cells: Vec<u32>,
    /// Synapses, packed by [`pack`]: from cell, to cell, connected.
    pub synapses: String,
    pub synapse_count: usize,
    /// The cells of each memory.
    pub engrams: Vec<(u64, Vec<u32>)>,
}

fn varint(bytes: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            bytes.push(byte);
            break;
        }
        bytes.push(byte | 0x80);
    }
}

fn varints(packed: &str) -> Vec<u64> {
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(packed) else {
        return Vec::new();
    };
    let mut values = Vec::new();
    let mut value = 0u64;
    let mut shift = 0;
    for byte in bytes {
        if shift >= 64 {
            return Vec::new();
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            values.push(value);
            value = 0;
            shift = 0;
        } else {
            shift += 7;
        }
    }
    values
}

/// Cell lists as sorted varint deltas in base64, about a byte and a half a
/// cell: the view is sent every step of the last few seconds.
mod packed {
    use base64::Engine as _;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(cells: &[u32], serializer: S) -> Result<S::Ok, S::Error> {
        let mut sorted = cells.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        let mut bytes = Vec::with_capacity(sorted.len() * 2);
        let mut last = 0;
        for cell in sorted {
            super::varint(&mut bytes, u64::from(cell - last));
            last = cell;
        }
        serializer.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u32>, D::Error> {
        let text = String::deserialize(deserializer)?;
        let mut cell = 0u32;
        Ok(super::varints(&text)
            .into_iter()
            .map(|delta| {
                cell = cell.saturating_add(delta as u32);
                cell
            })
            .collect())
    }
}

/// Packs synapses (from, to, connected) as varint deltas, sorted by
/// target, in base64: a few bytes each, so a million fit in one reply.
pub fn pack(mut synapses: Vec<(u32, u32, bool)>) -> String {
    synapses.sort_unstable_by_key(|s| (s.1, s.0));
    synapses.dedup_by_key(|s| (s.1, s.0));
    let mut bytes = Vec::with_capacity(synapses.len() * 4);
    let mut last_to = 0u32;
    for (from, to, connected) in synapses {
        varint(&mut bytes, u64::from(to - last_to));
        varint(&mut bytes, (u64::from(from) << 1) | u64::from(connected));
        last_to = to;
    }
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// The synapses [`pack`] packed.
pub fn unpack(packed: &str) -> Vec<(u32, u32, bool)> {
    let mut to = 0u32;
    varints(packed)
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            to = to.saturating_add(pair[0] as u32);
            ((pair[1] >> 1) as u32, to, pair[1] & 1 == 1)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synapses_pack_and_unpack() {
        let synapses = vec![(5, 9, true), (70_000, 3, false), (1, 9, false), (2, 1_000_000, true)];
        let mut expected = synapses.clone();
        expected.sort_by_key(|s| (s.1, s.0));
        assert_eq!(unpack(&pack(synapses)), expected);
    }

    #[test]
    fn activity_cells_travel_packed() {
        let activity = Activity {
            cells: vec![9, 3, 70_000],
            primed: Vec::new(),
            ..Activity::default()
        };
        let back: Activity = serde_json::from_str(&serde_json::to_string(&activity).unwrap()).unwrap();
        assert_eq!(back.cells, vec![3, 9, 70_000]);
        assert!(back.primed.is_empty());
    }

    #[test]
    fn surprise_cuts_experience_into_episodes_and_stretches_time() {
        let mut felt = Felt::default();
        for at in 0..20 {
            felt.tick("learn", "same", 0.0, true, at);
        }
        let routine = felt.moments;
        felt.tick("learn", "new", 1.0, true, 20);
        assert_eq!(felt.episodes.len(), 1, "{felt:?}");
        assert!(felt.moments - routine >= 2.9);
        for at in 0..20 {
            felt.tick("replay", "old", 0.0, false, 30 + at);
        }
        assert!(felt.now.felt < 2.0, "replay runs compressed: {:?}", felt.now);
        assert_eq!(felt.now.process, "replay");
    }

    #[test]
    fn it_learns_how_far_its_sureness_can_be_trusted() {
        let mut me = SelfModel::default();
        for at in 0..40 {
            me.guessed(0.9, at % 10 != 0);
            me.guessed(0.2, at % 10 == 0);
        }
        assert!(me.expect(0.95).unwrap() > 0.8);
        assert!(me.expect(0.25).unwrap() < 0.2);
        assert!(me.insight().unwrap() > 0.5);
    }
}
