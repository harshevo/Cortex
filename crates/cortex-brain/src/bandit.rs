//! Choosing which strategies a run's attempts are given.
//!
//! When a task is attempted several times, each attempt after the first
//! is told to go about it differently (`cortex_core::rollout`). Which
//! ways work depends on the repository and the model, so the choice is
//! learned: a Thompson-sampling bandit keeps, for each strategy, how often
//! an attempt given it produced convincing work, and tries the ones that
//! have done well more often while still trying the others sometimes.
//!
//! The plain attempt always goes first. It is the control, and with
//! attempts made one after another it is usually the only one made.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use cortex_core::rollout::{self, Strategy, StrategyPolicy};
use serde::{Deserialize, Serialize};

/// How often a strategy was tried and how often it worked.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub convincing: u32,
    pub unconvincing: u32,
}

impl Record {
    pub fn tried(&self) -> u32 {
        self.convincing + self.unconvincing
    }

    /// The expected share of convincing attempts, starting from an even
    /// guess before anything is known.
    pub fn mean(&self) -> f64 {
        (f64::from(self.convincing) + 1.0) / (f64::from(self.tried()) + 2.0)
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    records: BTreeMap<String, Record>,

    /// Advances with every draw, so that two runs do not draw alike.
    draws: u64,
}

#[derive(Debug)]
pub struct StrategyBandit {
    state: Mutex<State>,

    /// Where the records are kept between runs.
    file: Option<PathBuf>,
}

impl StrategyBandit {
    /// A bandit that starts from the records saved in `file`, if any.
    pub fn new(file: Option<PathBuf>) -> Self {
        let state = file
            .as_ref()
            .and_then(|file| fs::read_to_string(file).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();

        Self {
            state: Mutex::new(state),
            file,
        }
    }

    pub fn records(&self) -> BTreeMap<String, Record> {
        self.state().records.clone()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn save(&self, state: &State) -> io::Result<()> {
        let Some(file) = &self.file else {
            return Ok(());
        };

        if let Some(parent) = file.parent() {
            fs::create_dir_all(parent)?;
        }

        fs::write(file, serde_json::to_string_pretty(state)?)
    }
}

impl StrategyPolicy for StrategyBandit {
    fn choose(&self, count: usize) -> Vec<Strategy> {
        let mut state = self.state();

        let mut strategies = rollout::strategies();

        let plain = strategies.remove(0);

        // One draw per strategy from what is believed about it; the
        // highest draws are tried. A strategy with a short record draws
        // widely, which is what gets it tried.
        let mut drawn: Vec<(f64, Strategy)> = strategies
            .into_iter()
            .map(|strategy| {
                let record = state
                    .records
                    .get(&strategy.name)
                    .copied()
                    .unwrap_or_default();

                state.draws += 1;

                let mut random = Random::seeded(state.draws);

                let draw = random.beta(
                    f64::from(record.convincing) + 1.0,
                    f64::from(record.unconvincing) + 1.0,
                );

                (draw, strategy)
            })
            .collect();

        drawn.sort_by(|left, right| right.0.total_cmp(&left.0));

        std::iter::once(plain)
            .chain(drawn.into_iter().map(|(_, strategy)| strategy))
            .cycle()
            .take(count)
            .collect()
    }

    fn learn(&self, tried: &[(String, bool)]) {
        let mut state = self.state();

        for (name, convincing) in tried {
            let record = state.records.entry(name.clone()).or_default();

            if *convincing {
                record.convincing += 1;
            } else {
                record.unconvincing += 1;
            }
        }

        if let Err(error) = self.save(&state) {
            eprintln!("warning: the strategy records could not be saved: {error}");
        }
    }
}

/// A small random number generator. The draws only have to differ from
/// one another; nothing here needs to be unpredictable.
struct Random {
    state: u64,
}

impl Random {
    fn seeded(seed: u64) -> Self {
        Self {
            state: seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1,
        }
    }

    /// Evenly spread over (0, 1).
    fn uniform(&mut self) -> f64 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;

        ((self.state >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    /// A draw from the gamma distribution with a whole-number shape: the
    /// sum of that many exponential draws.
    fn gamma(&mut self, shape: f64) -> f64 {
        (0..shape.round().max(1.0) as u32)
            .map(|_| -self.uniform().ln())
            .sum()
    }

    /// A draw from the beta distribution with whole-number parameters.
    fn beta(&mut self, alpha: f64, beta: f64) -> f64 {
        let left = self.gamma(alpha);
        let right = self.gamma(beta);

        left / (left + right)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(strategies: &[Strategy]) -> Vec<&str> {
        strategies
            .iter()
            .map(|strategy| strategy.name.as_str())
            .collect()
    }

    #[test]
    fn the_plain_attempt_always_goes_first() {
        let bandit = StrategyBandit::new(None);

        for _ in 0..20 {
            let chosen = bandit.choose(3);

            assert_eq!(chosen.len(), 3);
            assert_eq!(chosen[0].name, "plain");
            assert_ne!(chosen[1].name, chosen[2].name);
        }
    }

    #[test]
    fn a_strategy_that_keeps_working_is_tried_most() {
        let bandit = StrategyBandit::new(None);

        for _ in 0..30 {
            bandit.learn(&[
                ("reproduce_first".to_string(), true),
                ("read_widely".to_string(), false),
                ("smallest_patch".to_string(), false),
            ]);
        }

        let second: Vec<String> = (0..50).map(|_| bandit.choose(2)[1].name.clone()).collect();

        let reproduce = second
            .iter()
            .filter(|name| *name == "reproduce_first")
            .count();

        assert!(
            reproduce >= 45,
            "chosen second only {reproduce} times in 50"
        );
    }

    #[test]
    fn an_untried_strategy_still_gets_its_turn() {
        let bandit = StrategyBandit::new(None);

        // One strategy has a middling record; the others have none.
        for round in 0..10 {
            bandit.learn(&[("reproduce_first".to_string(), round % 2 == 0)]);
        }

        let second: Vec<String> = (0..60).map(|_| bandit.choose(2)[1].name.clone()).collect();

        for name in ["read_widely", "smallest_patch"] {
            assert!(
                second.iter().any(|chosen| chosen == name),
                "{name} was never tried"
            );
        }
    }

    #[test]
    fn records_are_kept_between_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("memory.strategies.json");

        StrategyBandit::new(Some(file.clone())).learn(&[("plain".to_string(), true)]);

        let records = StrategyBandit::new(Some(file)).records();

        assert_eq!(records["plain"].convincing, 1);
        assert_eq!(names(&StrategyBandit::new(None).choose(1)), ["plain"]);
    }
}
