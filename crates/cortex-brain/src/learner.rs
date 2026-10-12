//! Learning what each action is worth in a situation.
//!
//! The rule is the one attributed to dopamine neurons: after an outcome,
//! compare the reward received with the reward expected, and move the
//! expectation by a fraction of the difference (the reward prediction
//! error). Only the weights from active inputs to the action that was
//! taken change, as at the synapses of the fly's mushroom body.

use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::situation::Action;

const ACTIONS: usize = Action::ALL.len();

/// Fraction of a prediction error applied per update.
const LEARNING_RATE: f32 = 0.2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Learner {
    /// Length of the codes this learner was trained on.
    dimension: usize,

    /// Runs with a checked result that have been learned from.
    runs: u32,

    /// One weight vector per action, in the order of [`Action::ALL`].
    weights: Vec<Vec<f32>>,
}

impl Learner {
    pub fn new(dimension: usize) -> Self {
        Self {
            dimension,
            runs: 0,
            weights: vec![vec![0.0; dimension]; ACTIONS],
        }
    }

    pub fn runs(&self) -> u32 {
        self.runs
    }

    pub fn dimension(&self) -> usize {
        self.dimension
    }

    /// The reward expected from taking `action` when the situation's code
    /// is `code`.
    pub fn value(&self, action: Action, code: &[f32]) -> f32 {
        self.weights[action.index()]
            .iter()
            .zip(code)
            .map(|(weight, activity)| weight * activity)
            .sum()
    }

    pub fn values(&self, code: &[f32]) -> [f32; ACTIONS] {
        Action::ALL.map(|action| self.value(action, code))
    }

    /// Moves the expected reward of `action` towards `reward`. Returns the
    /// prediction error.
    pub fn learn(&mut self, code: &[f32], action: Action, reward: f32) -> f32 {
        let error = reward - self.value(action, code);

        for (weight, activity) in self.weights[action.index()].iter_mut().zip(code) {
            *weight += LEARNING_RATE * error * activity;
        }

        error
    }

    pub fn finish_run(&mut self) {
        self.runs += 1;
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        crate::persistence::save(path, self)
    }

    /// The saved learner, or a new one if there is none or it was trained
    /// on codes of a different length.
    pub fn load_or_new(path: &Path, dimension: usize) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<Self>(&text).ok())
            .filter(|learner| learner.dimension == dimension)
            .unwrap_or_else(|| Self::new(dimension))
    }
}

/// Scales a code to unit length, so that one update moves a prediction by
/// the same fraction whatever the code's size.
pub fn normalize(code: &mut [f32]) {
    let length = code.iter().map(|value| value * value).sum::<f32>().sqrt();

    if length > 0.0 {
        for value in code {
            *value /= length;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_expected_reward_moves_towards_what_was_received() {
        let mut learner = Learner::new(2);

        let code = [1.0, 0.0];

        assert_eq!(learner.value(Action::Run, &code), 0.0);

        let first_error = learner.learn(&code, Action::Run, 1.0);

        for _ in 0..30 {
            learner.learn(&code, Action::Run, 1.0);
        }

        assert_eq!(first_error, 1.0);
        assert!((learner.value(Action::Run, &code) - 1.0).abs() < 0.01);

        // Other actions and unrelated situations are untouched.
        assert_eq!(learner.value(Action::Edit, &code), 0.0);
        assert_eq!(learner.value(Action::Run, &[0.0, 1.0]), 0.0);
    }

    #[test]
    fn a_learner_survives_being_saved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state/learner.json");

        let mut learner = Learner::new(2);

        learner.learn(&[1.0, 0.0], Action::Edit, 1.0);
        learner.finish_run();
        learner.save(&path).unwrap();

        let loaded = Learner::load_or_new(&path, 2);

        assert_eq!(loaded.runs(), 1);
        assert!(loaded.value(Action::Edit, &[1.0, 0.0]) > 0.0);

        // A different code length means a different encoder: start again.
        assert_eq!(Learner::load_or_new(&path, 3).runs(), 0);
    }
}
