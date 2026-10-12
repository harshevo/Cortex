//! Deciding how sure a hunch has to be before it is said.
//!
//! A fixed threshold is a guess. The network's strengths mean different
//! things in different repositories: 0.5 may be right nine times in ten in
//! one and one time in three in another. So the threshold is set from the
//! record. Every file the network foresaw at the start of a run is kept
//! with its strength and with whether the run went on to change that
//! file, and the threshold is the lowest strength above which the record
//! was right often enough.
//!
//! The rule is the one from conformal risk control: with `k` foreseen
//! files at or above a strength and `w` of them wrong, the threshold is
//! allowed there only if `(w + 1) / (k + 1)` is within the error allowed.
//! The `+ 1` is what makes a short record cautious: three hunches all
//! right are not yet proof of anything better than three in four.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

/// Hunches remembered. Older ones are forgotten, so the threshold follows
/// a network that is still learning.
const MAX_RECORDS: usize = 200;

/// The record must be at least this long before it replaces the fixed
/// threshold.
const MIN_RECORDS: usize = 10;

/// A strength that no hunch reaches: say nothing.
const SILENT: f32 = 1.01;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Calibration {
    /// Strength of a foreseen file, and whether the run changed it.
    records: VecDeque<(f32, bool)>,
}

impl Calibration {
    pub fn add(&mut self, strength: f32, right: bool) {
        self.records.push_back((strength, right));

        while self.records.len() > MAX_RECORDS {
            self.records.pop_front();
        }
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// The lowest strength at which hunches have been right at least
    /// `precision` of the time, or `fallback` while the record is short.
    ///
    /// When no strength has been right that often, the answer is above
    /// every possible strength, so nothing is said.
    pub fn threshold(&self, precision: f32, fallback: f32) -> f32 {
        if self.records.len() < MIN_RECORDS {
            return fallback;
        }

        let mut sorted: Vec<(f32, bool)> = self.records.iter().copied().collect();

        sorted.sort_by(|left, right| right.0.total_cmp(&left.0));

        let allowed = 1.0 - precision;

        let mut wrong = 0;
        let mut threshold = SILENT;

        for (position, (strength, right)) in sorted.iter().enumerate() {
            if !right {
                wrong += 1;
            }

            // Hunches of equal strength stand or fall together.
            let last_of_its_strength = sorted
                .get(position + 1)
                .is_none_or(|(next, _)| next < strength);

            let bound = (wrong + 1) as f32 / (position + 2) as f32;

            if last_of_its_strength && bound <= allowed {
                threshold = *strength;
            }
        }

        threshold
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(entries: &[(f32, bool)]) -> Calibration {
        let mut calibration = Calibration::default();

        for (strength, right) in entries {
            calibration.add(*strength, *right);
        }

        calibration
    }

    #[test]
    fn a_short_record_keeps_the_fixed_threshold() {
        let calibration = record(&[(0.9, true), (0.8, true), (0.3, false)]);

        assert_eq!(calibration.threshold(0.7, 0.5), 0.5);
    }

    #[test]
    fn the_threshold_falls_to_where_hunches_stop_being_right() {
        // Right down to 0.4, wrong below it.
        let calibration = record(&[
            (0.9, true),
            (0.85, true),
            (0.8, true),
            (0.7, true),
            (0.6, true),
            (0.5, true),
            (0.45, true),
            (0.4, true),
            (0.35, false),
            (0.3, false),
            (0.25, false),
            (0.2, false),
        ]);

        // At 0.35 the bound is 2/10; at 0.3 it is 3/11, still within 0.3;
        // at 0.25 it is 4/12, which is not.
        assert_eq!(calibration.threshold(0.7, 0.5), 0.3);
        assert_eq!(calibration.threshold(0.85, 0.5), 0.4);
    }

    #[test]
    fn a_network_that_is_mostly_wrong_is_silenced() {
        let entries: Vec<(f32, bool)> = (0..12)
            .map(|step| (0.9 - step as f32 * 0.05, step % 3 == 0))
            .collect();

        assert_eq!(record(&entries).threshold(0.7, 0.5), SILENT);
    }

    #[test]
    fn old_records_are_forgotten() {
        let mut calibration = Calibration::default();

        for _ in 0..MAX_RECORDS + 50 {
            calibration.add(0.5, true);
        }

        assert_eq!(calibration.len(), MAX_RECORDS);
    }
}
