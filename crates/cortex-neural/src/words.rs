//! How words reach the cortex: each word fires its own set of columns, a
//! sparse code. Most of a word's columns are its identity and never move.
//! The rest drift, during sleep, towards the columns of the words it is
//! met beside, so that words used alike come to share columns and what is
//! learned about one carries over to the other.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

/// Columns in the cortex.
pub const COLUMNS: usize = 2048;

/// Columns each word fires.
pub const ACTIVE: usize = 40;

/// Of those, the ones that are the word's identity and never drift.
const IDENTITY: usize = 24;

/// Most words it keeps codes for.
pub const VOCABULARY: usize = 20_000;

/// Context votes kept per word.
const VOTES: usize = 96;

/// Marks the turns of a conversation and the start of a story.
pub const USER: &str = "<user>";
pub const BRAIN: &str = "<brain>";
pub const STORY: &str = "<story>";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Word {
    /// Sorted, `ACTIVE` long; the first `IDENTITY` (before sorting) are kept
    /// apart in `identity`.
    pub columns: Vec<u16>,
    identity: Vec<u16>,
    pub seen: u64,

    /// Columns of the words met next to it, and how often.
    votes: HashMap<u16, f32>,

    /// How surprised the cortex was when it arrived, on average, now and
    /// as it was a while ago: their difference is learning progress.
    pub surprise: f32,
    pub surprise_before: f32,
    pub drifted: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Words {
    pub words: BTreeMap<String, Word>,
}

pub fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    for raw in text.split_whitespace() {
        let lower = raw.to_lowercase();
        if matches!(lower.as_str(), USER | BRAIN | STORY) {
            tokens.push(lower);
            continue;
        }
        let mut word = String::new();
        for c in lower.chars() {
            if c.is_alphanumeric() || c == '\'' {
                word.push(c);
            } else {
                if !word.is_empty() {
                    tokens.push(std::mem::take(&mut word));
                }
                if matches!(c, '.' | '?' | '!' | ',') {
                    tokens.push(c.to_string());
                }
            }
        }
        if !word.is_empty() {
            tokens.push(word);
        }
    }
    tokens.retain(|t| t.chars().count() <= 32);
    tokens
}

/// Whether a token ends a sentence.
pub fn ends_sentence(token: &str) -> bool {
    matches!(token, "." | "?" | "!" | USER)
}

fn hash(text: &str, salt: u64) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325_u64 ^ salt.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    for b in text.bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^ (h >> 33)
}

impl Words {
    pub fn len(&self) -> usize {
        self.words.len()
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    /// The columns of `token`, giving it a code if it is new. `None` when
    /// the vocabulary is full and the word is unknown.
    pub fn columns(&mut self, token: &str) -> Option<Vec<u16>> {
        if let Some(word) = self.words.get(token) {
            return Some(word.columns.clone());
        }
        if self.words.len() >= VOCABULARY {
            return None;
        }
        let mut columns = Vec::with_capacity(ACTIVE);
        let mut salt = 0;
        while columns.len() < ACTIVE {
            let column = (hash(token, salt) % COLUMNS as u64) as u16;
            if !columns.contains(&column) {
                columns.push(column);
            }
            salt += 1;
        }
        let identity = columns[..IDENTITY].to_vec();
        columns.sort_unstable();
        self.words.insert(
            token.to_string(),
            Word {
                columns: columns.clone(),
                identity,
                seen: 0,
                votes: HashMap::new(),
                surprise: 1.0,
                surprise_before: 1.0,
                drifted: 0,
            },
        );
        Some(columns)
    }

    /// Counts a word as met, beside these neighbours, with this surprise.
    pub fn met(&mut self, token: &str, neighbours: &[&str], surprise: f32) {
        let columns: Vec<Vec<u16>> = neighbours
            .iter()
            .filter(|n| **n != token)
            .filter_map(|n| self.words.get(*n).map(|w| w.columns.clone()))
            .collect();
        let Some(word) = self.words.get_mut(token) else {
            return;
        };
        word.seen += 1;
        word.surprise += 0.2 * (surprise - word.surprise);
        word.surprise_before += 0.02 * (surprise - word.surprise_before);
        for column in columns.iter().flatten() {
            *word.votes.entry(*column).or_default() += 1.0;
        }
        if word.votes.len() > VOTES * 2 {
            let mut votes: Vec<(u16, f32)> = word.votes.drain().collect();
            votes.sort_by(|a, b| b.1.total_cmp(&a.1));
            votes.truncate(VOTES);
            word.votes = votes.into_iter().collect();
        }
    }

    /// Lets each word's drifting columns move one step towards its
    /// neighbours' columns. Returns how many words drifted.
    pub fn drift(&mut self) -> usize {
        let mut drifted = 0;
        for word in self.words.values_mut() {
            if word.seen < 3 || word.votes.is_empty() {
                continue;
            }
            let best = word
                .votes
                .iter()
                .filter(|(c, _)| !word.columns.contains(c))
                .max_by(|a, b| a.1.total_cmp(b.1).then(b.0.cmp(a.0)))
                .map(|(c, v)| (*c, *v));
            let Some((column, votes)) = best else {
                continue;
            };
            if votes < 2.0 {
                continue;
            }
            // The drifting column with the fewest votes gives way.
            let weakest = word
                .columns
                .iter()
                .filter(|c| !word.identity.contains(c))
                .min_by(|a, b| {
                    let va = word.votes.get(a).copied().unwrap_or(0.0);
                    let vb = word.votes.get(b).copied().unwrap_or(0.0);
                    va.total_cmp(&vb).then(a.cmp(b))
                })
                .copied();
            if let Some(weakest) = weakest
                && word.votes.get(&weakest).copied().unwrap_or(0.0) < votes * 0.5
            {
                word.columns.retain(|c| *c != weakest);
                word.columns.push(column);
                word.columns.sort_unstable();
                word.drifted += 1;
                drifted += 1;
            }
            for vote in word.votes.values_mut() {
                *vote *= 0.9;
            }
        }
        drifted
    }

    /// Words scored by how much of their code lies in `predicted` (a count
    /// of predicted cells per column), best first.
    pub fn read(&self, predicted: &HashMap<u16, u32>, most: usize) -> Vec<(String, f32, usize)> {
        if predicted.is_empty() {
            return Vec::new();
        }
        let mut scored: Vec<(String, f32, usize)> = self
            .words
            .iter()
            .filter_map(|(text, word)| {
                let hits = word.columns.iter().filter(|c| predicted.contains_key(c)).count();
                (hits > 0).then(|| (text.clone(), hits as f32 / ACTIVE as f32, hits))
            })
            .collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        scored.truncate(most);
        scored
    }

    /// How many columns two words share.
    pub fn shared(&self, one: &str, other: &str) -> usize {
        match (self.words.get(one), self.words.get(other)) {
            (Some(a), Some(b)) => a.columns.iter().filter(|c| b.columns.contains(c)).count(),
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_keep_punctuation_and_turn_marks() {
        assert_eq!(
            tokenize("<user> Hi, Sam! It's 5."),
            vec!["<user>", "hi", ",", "sam", "!", "it's", "5", "."]
        );
    }

    #[test]
    fn every_word_fires_its_own_columns_and_alike_words_come_to_share() {
        let mut words = Words::default();
        let cat = words.columns("cat").unwrap();
        assert_eq!(cat.len(), ACTIVE);
        assert_eq!(words.columns("cat").unwrap(), cat);
        for token in ["the", "sat", "dog", "ran"] {
            words.columns(token);
        }
        let before = words.shared("cat", "dog");
        for _ in 0..40 {
            words.met("cat", &["the", "sat"], 0.5);
            words.met("dog", &["the", "sat"], 0.5);
            words.drift();
        }
        assert!(words.shared("cat", "dog") > before);
        assert!(words.shared("cat", "dog") <= ACTIVE - IDENTITY);
    }
}
