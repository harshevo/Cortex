//! Frozen prediction measurements and bounded, criterion-based rehearsal.

use super::{
    Brain,
    cortex::probe::Probe,
    words::{BRAIN, STORY, USER, Words, tokenize},
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Assessment {
    pub tokens: u64,
    pub correct: u64,
    pub baseline_correct: u64,
    pub predictions: u64,
    pub accuracy: f32,
    pub baseline_accuracy: f32,
    pub coverage: f32,
    pub brier: Option<f32>,
    pub calibration_samples: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Study {
    pub memory: Option<u64>,
    pub passes: usize,
    pub before: Assessment,
    pub after: Assessment,
    pub target: f32,
    pub reached: bool,
    pub updates: u64,
}

impl Brain {
    /// Explicit opt-in lessons; these are supplied knowledge, not discoveries.
    pub fn foundations(&mut self) -> usize {
        let mut count = 0;
        for line in include_str!("foundations.jsonl").lines() {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(line)
                && let Some(text) = value["text"].as_str()
            {
                self.study(text, "bundled introductory curriculum", 4);
                count += 1;
            }
        }
        count
    }

    /// Read-only inference receives the observations; the original learns nothing,
    /// including no calibration, vocabulary, RNG, or usage-statistic changes.
    pub fn assess(&self, texts: &[&str]) -> Assessment {
        let mut cortex = Probe::new(&self.cortex);
        let mut assessment = Assessment::default();
        let commonest = self.commonest();
        let mut squared_error = 0.0;
        for text in texts {
            cortex.reset();
            for (at, token) in tokenize(text).iter().take(2048).enumerate() {
                if at > 0 && !matches!(token.as_str(), USER | BRAIN | STORY) {
                    assessment.tokens += 1;
                    assessment.baseline_correct += u64::from(commonest.as_ref() == Some(token));
                    let guesses = self.words.read(&cortex.primed_columns(), 1);
                    let correct = guesses.first().is_some_and(|g| g.0 == *token);
                    assessment.correct += u64::from(correct);
                    if let Some(guess) = guesses.first() {
                        assessment.predictions += 1;
                        if let Some(probability) = self.mind.me.expect(guess.1) {
                            assessment.calibration_samples += 1;
                            squared_error +=
                                (probability - if correct { 1.0 } else { 0.0 }).powi(2);
                        }
                    }
                }
                let columns = self
                    .words
                    .words
                    .get(token)
                    .map(|w| w.columns.clone())
                    .or_else(|| Words::default().columns(token, self.cortex.area()));
                if let Some(columns) = columns {
                    cortex.step(&columns);
                }
            }
        }
        let total = assessment.tokens.max(1) as f32;
        assessment.accuracy = assessment.correct as f32 / total;
        assessment.baseline_accuracy = assessment.baseline_correct as f32 / total;
        assessment.coverage = assessment.predictions as f32 / total;
        assessment.brier = (assessment.calibration_samples > 0)
            .then(|| squared_error / assessment.calibration_samples as f32);
        assessment
    }

    /// Rehearsal stops on this lesson's criterion or the fixed pass budget.
    /// This measures retention of taught material, never held-out accuracy.
    pub fn study(&mut self, text: &str, source: &str, max_passes: usize) -> Study {
        let mut study = Study {
            before: self.assess(&[text]),
            target: 0.85,
            ..Study::default()
        };
        let before_updates = self.cortex.totals.strengthened + self.cortex.totals.synapses_grown;
        for _ in 0..max_passes.clamp(1, 8) {
            study.memory = self.learn(text, source);
            study.passes += 1;
            study.after = self.assess(&[text]);
            if study.memory.is_none() || study.after.accuracy >= study.target {
                break;
            }
        }
        study.reached = study.after.tokens > 0 && study.after.accuracy >= study.target;
        study.updates = (self.cortex.totals.strengthened + self.cortex.totals.synapses_grown)
            .saturating_sub(before_updates);
        self.mind.cognition.last_study = Some(study.clone());
        study
    }

    /// Branch-only fast inference: no explicit knowledge or arithmetic engine.
    pub fn predict(&self, cue: &str, most: usize) -> Vec<(String, f32, u32)> {
        let mut cortex = Probe::new(&self.cortex);
        cortex.reset();
        for token in tokenize(cue).iter().take(256) {
            let columns = self
                .words
                .words
                .get(token)
                .map(|w| w.columns.clone())
                .or_else(|| Words::default().columns(token, self.cortex.area()));
            if let Some(columns) = columns {
                cortex.step(&columns);
            }
        }
        self.words
            .read(&cortex.primed_columns(), most.min(16))
            .into_iter()
            .map(|(w, s, h)| (w, s, h as u32))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rehearsal_improves_prediction_without_falsifying_fresh_accuracy() {
        let mut brain = Brain::new();
        let text = "<story> the fox walked through the forest .";
        let study = brain.study(text, "development", 8);
        assert!(study.after.accuracy > study.before.accuracy, "{study:?}");
        assert!(study.after.accuracy >= 0.7, "{study:?}");
        // Only first exposure is counted as fresh, irrespective of passes.
        assert_eq!(brain.stats.scored, tokenize(text).len() as u64 - 1);
        let bytes = postcard::to_stdvec(&brain).unwrap();
        let mind = brain.mind.clone();
        let result = brain.assess(&["<story> the fox walked through the forest ."]);
        assert!(result.accuracy > result.baseline_accuracy);
        brain.predict("<story> the fox", 4);
        assert_eq!(postcard::to_stdvec(&brain).unwrap(), bytes);
        assert_eq!(brain.mind, mind);
    }
}
