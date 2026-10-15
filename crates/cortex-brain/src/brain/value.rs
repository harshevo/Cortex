//! What matters, and what is held in mind.
//!
//! [`Values`] is the brain's sense of good and bad. Some things are news
//! by nature (the checks passing, the person saying no, a trade accepted):
//! their worth is given. Everything else learns its worth from what
//! followed it: when news comes, the things that happened in the moments
//! before take some of it, the nearest the most. This is the credit a
//! dopamine signal assigns along an eligibility trace, done plainly.
//!
//! [`Attention`] is the brain's workspace: a few things held for a while
//! because they were new, surprising or mattered. What it holds pulls the
//! brain's choices towards itself when it speaks or imagines, and is what
//! it says it was attending to.

use std::collections::{HashMap, VecDeque};

use serde::{Deserialize, Serialize};

/// Moments back that news reaches, and how much less each earlier one
/// takes.
const TRACE: usize = 8;
const CREDIT: f32 = 0.6;

/// How far a thing's worth moves toward news that follows it at once.
const RATE: f32 = 0.2;

/// Things held in mind at once, and the share of its salience a held
/// thing keeps from one moment to the next.
const HELD: usize = 4;
const HOLD: f32 = 0.6;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Values {
    /// The worth of news, given rather than learned, from -1 to 1.
    pub meanings: HashMap<String, f32>,

    /// The worth each other thing has learned from the news that followed
    /// it, from -1 to 1.
    pub learned: HashMap<String, f32>,
}

/// The things of the last few moments of one stream, newest last: who
/// takes credit when news comes.
pub type Trace = VecDeque<Vec<String>>;

impl Values {
    /// Gives these things their worth, keeping what was given before.
    pub fn mean(&mut self, meanings: &[(&str, f32)]) {
        for (token, worth) in meanings {
            self.meanings
                .entry((*token).to_string())
                .or_insert(worth.clamp(-1.0, 1.0));
        }
    }

    /// The worth of a thing: given, or learned.
    pub fn worth(&self, token: &str) -> f32 {
        self.meanings
            .get(token)
            .or_else(|| self.learned.get(token))
            .copied()
            .unwrap_or(0.0)
    }

    /// Whether this is news by nature.
    pub fn is_news(&self, token: &str) -> bool {
        self.meanings.contains_key(token)
    }

    /// One moment lived, after the moments in `trace`. When it holds news,
    /// the moments before take their share. Returns the news, 0 when there
    /// was none.
    pub fn lived(&mut self, trace: &mut Trace, tokens: &[String], plasticity: f32) -> f32 {
        let news: f32 = tokens
            .iter()
            .filter_map(|token| self.meanings.get(token))
            .sum::<f32>()
            .clamp(-1.0, 1.0);
        if news != 0.0 {
            let mut rate = RATE * plasticity;
            for moment in trace.iter().rev() {
                for token in moment {
                    if self.meanings.contains_key(token) {
                        continue;
                    }
                    let value = self.learned.entry(token.clone()).or_insert(0.0);
                    *value += rate * (news - *value);
                }
                rate *= CREDIT;
            }
        }
        trace.push_back(tokens.to_vec());
        while trace.len() > TRACE {
            trace.pop_front();
        }
        news
    }

    /// Learned worths that have faded to nothing are let go.
    pub fn tidy(&mut self) {
        self.learned.retain(|_, value| value.abs() >= 0.01);
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Attention {
    /// What is held, most salient first, with its salience.
    pub held: Vec<(String, f32)>,
}

impl Attention {
    /// A thing met, with how much it stands out. What is held fades, and
    /// the most salient few are kept.
    pub fn met(&mut self, token: &str, salience: f32) {
        for (_, held) in &mut self.held {
            *held *= HOLD;
        }
        match self.held.iter_mut().find(|(held, _)| held == token) {
            Some((_, held)) => *held = held.max(salience),
            None => self.held.push((token.to_string(), salience)),
        }
        self.held.retain(|(_, salience)| *salience > 0.1);
        self.held
            .sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        self.held.truncate(HELD);
    }

    pub fn holds(&self, token: &str) -> bool {
        self.held.iter().any(|(held, _)| held == token)
    }

    pub fn tokens(&self) -> Vec<String> {
        self.held.iter().map(|(token, _)| token.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn news_reaches_back_to_what_led_to_it_the_nearest_most() {
        let mut values = Values::default();
        let mut trace = Trace::new();
        values.mean(&[("checks: passed", 1.0)]);
        values.lived(&mut trace, &["does: edit".into()], 1.0);
        values.lived(&mut trace, &["runs: tests".into()], 1.0);
        let news = values.lived(&mut trace, &["checks: passed".into()], 1.0);
        assert_eq!(news, 1.0);
        assert!(values.worth("runs: tests") > values.worth("does: edit"));
        assert!(values.worth("does: edit") > 0.0);
        assert_eq!(values.worth("checks: passed"), 1.0);
    }

    #[test]
    fn attention_holds_the_most_salient_few_and_lets_them_fade() {
        let mut attention = Attention::default();
        for (token, salience) in [("a", 1.0), ("b", 0.5), ("c", 0.3), ("d", 0.9), ("e", 0.8)] {
            attention.met(token, salience);
        }
        assert_eq!(attention.held.len(), 4);
        assert_eq!(attention.held[0].0, "e");
        assert!(!attention.holds("c"));
        for _ in 0..10 {
            attention.met("x", 0.2);
        }
        assert_eq!(attention.tokens(), vec!["x".to_string()]);
    }
}
