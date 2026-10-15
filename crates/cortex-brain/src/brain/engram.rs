//! Memories as the cells that held them.
//!
//! When something is learned, the cells that won in each step are kept
//! with it: its engram. The cells themselves are the cortex's; what the
//! engram keeps is which ones they were. Recall finds the engram whose
//! cells best overlap the cells a cue sets spiking (the index the
//! hippocampus is thought to keep), re-fires its first cells, and lets the
//! cortex's own synapses carry the sequence on. How many of the stored
//! cells come back on their own is the recall's fidelity.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Engram {
    pub id: u64,
    pub source: String,
    pub tokens: Vec<String>,

    /// The cells that won at each step.
    pub cells: Vec<Vec<u32>>,
    pub created: u64,
    pub strength: f32,
    pub recalls: u64,
    pub replays: u64,
    pub last_recalled: u64,

    /// Of the cells stored, the share that came back on the latest recall.
    pub fidelity: Option<f32>,
}

impl Engram {
    pub fn text(&self) -> String {
        join(&self.tokens)
    }

    pub fn cell_count(&self) -> usize {
        let mut all: Vec<u32> = self.cells.iter().flatten().copied().collect();
        all.sort_unstable();
        all.dedup();
        all.len()
    }
}

/// Tokens as text: no space before punctuation.
pub fn join(tokens: &[String]) -> String {
    let mut text = String::new();
    for token in tokens {
        if !text.is_empty() && !matches!(token.as_str(), "." | "," | "?" | "!") {
            text.push(' ');
        }
        text.push_str(token);
    }
    text
}

/// One recall, step by step, for the record.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Recall {
    pub time: u64,
    pub cue: String,
    pub engram: Option<u64>,
    pub remembered: String,

    /// Shared cells over cue cells, for the engram chosen, and for the
    /// next best.
    pub matched: f32,
    pub runner_up: f32,

    /// What came back, and of the stored cells, the share that did.
    pub recalled: String,
    pub fidelity: f32,
    pub words_right: u32,
    pub words: u32,

    /// The cells re-fired to start it, and some that came back after.
    pub reignited: Vec<u32>,
    pub returned: Vec<u32>,
    pub why: String,
}
