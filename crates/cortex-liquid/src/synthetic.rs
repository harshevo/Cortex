//! Made-up series for testing and for trying the analysis without a
//! file: one that holds a pattern a model should find, and one that holds
//! none, where the only honest result is "no edge".

use crate::bars::Bar;
use crate::rng::Rng;

const DAY: i64 = 86_400_000;

/// 2021-01-04, a Monday.
const START: i64 = 18_631 * DAY;

/// Daily bars with weekends left out, so the gaps between bars are
/// uneven, closing at the prices the given log returns lead to.
fn bars_from(steps: impl Iterator<Item = f64>, rng: &mut Rng) -> Vec<Bar> {
    let mut bars = Vec::new();
    let mut close = 100.0;
    let mut day = 0;

    for step in steps {
        // Days 5 and 6 of each week are the weekend.
        while day % 7 >= 5 {
            day += 1;
        }

        let open = close;

        close *= step.exp();

        let wick = 0.002 * rng.unit();

        bars.push(Bar {
            time_ms: START + day * DAY,
            open,
            high: open.max(close) * (1.0 + wick),
            low: open.min(close) * (1.0 - 0.002 * rng.unit()),
            close,
            volume: 1_000.0 * (0.4 * rng.normal() + 6.0 * step.abs()).exp(),
        });

        day += 1;
    }

    bars
}

/// A random walk: each return is drawn afresh and owes nothing to what
/// came before. Nothing can predict its direction.
pub fn random_walk(count: usize, seed: u64) -> Vec<Bar> {
    let mut rng = Rng::new(seed);
    let steps: Vec<f64> = (0..count).map(|_| 0.01 * rng.normal()).collect();

    bars_from(steps.into_iter(), &mut rng)
}

/// A series whose returns depend on the two before: after two moves the
/// same way it tends to turn, after two moves opposite ways it tends to
/// go on as the later one did, with noise on top. The best possible
/// caller of its direction is right about five times in six; "same as
/// last bar" is right about half the time.
pub fn patterned(count: usize, seed: u64) -> Vec<Bar> {
    let mut rng = Rng::new(seed);
    let mut steps: Vec<f64> = Vec::with_capacity(count);

    for at in 0..count {
        let lean = if at < 2 {
            0.0
        } else {
            // Both cases come to: the opposite of the move two bars ago.
            -steps[at - 2].signum()
        };

        steps.push(0.006 * lean + 0.006 * rng.normal());
    }

    bars_from(steps.into_iter(), &mut rng)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_are_usable_in_order_and_skip_weekends() {
        for bars in [random_walk(50, 1), patterned(50, 1)] {
            assert_eq!(bars.len(), 50);
            assert!(bars.iter().all(|bar| bar.problem().is_none()));
            assert!(
                bars.iter()
                    .all(|bar| bar.low <= bar.close && bar.close <= bar.high)
            );

            let gaps: Vec<i64> = bars
                .windows(2)
                .map(|two| two[1].time_ms - two[0].time_ms)
                .collect();

            assert!(gaps.iter().all(|gap| *gap == DAY || *gap == 3 * DAY));
            assert!(gaps.contains(&(3 * DAY)));
        }
    }
}
