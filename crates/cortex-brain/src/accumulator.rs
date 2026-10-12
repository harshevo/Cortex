//! Choosing an action by competition.
//!
//! Each action has an accumulator that fills at a rate set by the evidence
//! for it and drains by leak. The accumulators inhibit one another, so as
//! one pulls ahead it holds the others down. The first to reach the
//! threshold wins. This is the leaky competing accumulator used to model
//! decisions in insects and primates alike.
//!
//! Its useful property here is that speed tracks certainty: clear evidence
//! decides in a few steps, close evidence takes long or never reaches the
//! threshold. A decision that never reached it is one the controller
//! should keep to itself.

use crate::situation::Action;

const ACTIONS: usize = Action::ALL.len();

const THRESHOLD: f32 = 1.0;
const MAX_STEPS: u32 = 200;
const TIME_STEP: f32 = 0.1;
const LEAK: f32 = 0.2;
const INHIBITION: f32 = 2.0;

/// Evidence is scaled so that an expected reward of 1 with no rival
/// reaches the threshold comfortably.
const GAIN: f32 = 2.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    /// The action with the fullest accumulator.
    pub action: Action,

    /// Whether it reached the threshold, that is, whether the evidence was
    /// clear enough to decide.
    pub decided: bool,

    /// Steps taken. Fewer means clearer evidence.
    pub steps: u32,

    /// Where each accumulator stood at the end.
    pub levels: [f32; ACTIONS],
}

/// Runs the race. `evidence` is the expected reward of each action, in the
/// order of [`Action::ALL`].
pub fn compete(evidence: [f32; ACTIONS]) -> Decision {
    let mut levels = [0.0_f32; ACTIONS];
    let mut steps = 0;

    while steps < MAX_STEPS && levels.iter().all(|level| *level < THRESHOLD) {
        let total: f32 = levels.iter().sum();

        let mut next = levels;

        for (index, level) in levels.iter().enumerate() {
            let rivals = total - level;

            let change = GAIN * evidence[index].max(0.0) - LEAK * level - INHIBITION * rivals;

            next[index] = (level + TIME_STEP * change).max(0.0);
        }

        levels = next;
        steps += 1;
    }

    let winner = (0..ACTIONS)
        .max_by(|left, right| levels[*left].total_cmp(&levels[*right]))
        .unwrap_or(0);

    Decision {
        action: Action::ALL[winner],
        decided: levels[winner] >= THRESHOLD,
        steps,
        levels,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence(run: f32, inspect: f32) -> [f32; ACTIONS] {
        let mut evidence = [0.0; ACTIONS];

        evidence[Action::Run.index()] = run;
        evidence[Action::Inspect.index()] = inspect;

        evidence
    }

    #[test]
    fn clear_evidence_decides_quickly() {
        let clear = compete(evidence(0.9, 0.2));
        let close = compete(evidence(0.9, 0.8));

        assert_eq!(clear.action, Action::Run);
        assert!(clear.decided);

        assert_eq!(close.action, Action::Run);
        assert!(close.steps > clear.steps);
    }

    #[test]
    fn evenly_matched_or_absent_evidence_decides_nothing() {
        assert!(!compete(evidence(0.5, 0.5)).decided);
        assert!(!compete([0.0; ACTIONS]).decided);
    }
}
