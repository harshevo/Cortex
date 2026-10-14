//! What the networks are shown of each bar: a few numbers that do not
//! depend on the price level or the size of the volume, each worked out
//! from that bar and the ones before it and nothing later.
//!
//! Every number here looks back at most [`LOOKBACK`] bars, so the row for
//! the newest bar can be worked out from the last [`KEPT`] bars alone and
//! comes out the same as it would from the whole series. That is what
//! lets a kept model take new bars without the old ones.

use serde::{Deserialize, Serialize};

use crate::bars::Bar;

/// Numbers per bar.
pub const FEATURES: usize = 11;

/// What each number is, in the order of a row.
pub const NAMES: [&str; FEATURES] = [
    "return",
    "range",
    "close in range",
    "volume vs usual",
    "gap from last close",
    "return over 5",
    "return over 20",
    "volatility, recent vs longer",
    "oscillator",
    "distance from mean",
    "return in volatility units",
];

/// Where each number is in a row.
pub const RETURN: usize = 0;
pub const RANGE: usize = 1;
pub const POSITION: usize = 2;
pub const VOLUME: usize = 3;
pub const GAP: usize = 4;
pub const RETURN_5: usize = 5;
pub const RETURN_20: usize = 6;
pub const VOLATILITY: usize = 7;
pub const OSCILLATOR: usize = 8;
pub const DISTANCE: usize = 9;
pub const RETURN_IN_VOLATILITY: usize = 10;

/// The furthest back any number looks, in bars before the one described.
pub const LOOKBACK: usize = 50;

/// Bars a kept model holds on to so that it can describe the next one.
pub const KEPT: usize = LOOKBACK + 14;

const SHORT: usize = 5;
const MEDIUM: usize = 20;
const VOLATILITY_SHORT: usize = 10;
const OSCILLATOR_OVER: usize = 14;

/// A standardised number is cut off this far from the mean, so that one
/// freak bar cannot swamp a network.
const FURTHEST: f64 = 5.0;

pub type Row = [f64; FEATURES];

/// The log return into bar `at` from the bar before it. 0 for the first.
fn step(bars: &[Bar], at: usize) -> f64 {
    if at == 0 {
        return 0.0;
    }

    (bars[at].close / bars[at - 1].close).ln()
}

/// The log returns into the last `count` bars up to and including `at`,
/// fewer near the start of the series.
fn steps(bars: &[Bar], at: usize, count: usize) -> impl Iterator<Item = f64> + '_ {
    let first = (at + 1).saturating_sub(count).max(1);

    (first..=at).map(|at| step(bars, at))
}

/// The standard deviation of the last `count` returns, 0 with fewer
/// than two.
fn spread(bars: &[Bar], at: usize, count: usize) -> f64 {
    let (mut sum, mut squares, mut seen) = (0.0, 0.0, 0.0);

    for step in steps(bars, at, count) {
        sum += step;
        squares += step * step;
        seen += 1.0;
    }

    if seen < 2.0 {
        return 0.0;
    }

    let mean = sum / seen;

    (squares / seen - mean * mean).max(0.0).sqrt()
}

/// The numbers for bar `at`, from `bars[..=at]` only.
pub fn row(bars: &[Bar], at: usize) -> Row {
    let bar = &bars[at];
    let mut row = [0.0; FEATURES];

    row[RETURN] = step(bars, at);
    row[RANGE] = (bar.high - bar.low) / bar.close;

    if bar.high > bar.low {
        row[POSITION] = (bar.close - bar.low) / (bar.high - bar.low) - 0.5;
    }

    // Against the bars before this one, so that a loud bar is not
    // measured against itself.
    let before = &bars[at.saturating_sub(MEDIUM)..at];

    if !before.is_empty() && bar.volume > 0.0 {
        let usual = before.iter().map(|bar| bar.volume).sum::<f64>() / before.len() as f64;

        if usual > 0.0 {
            row[VOLUME] = (bar.volume / usual).ln();
        }
    }

    if at > 0 {
        row[GAP] = (bar.open / bars[at - 1].close).ln();
    }

    row[RETURN_5] = steps(bars, at, SHORT).sum();
    row[RETURN_20] = steps(bars, at, MEDIUM).sum();

    let recent = spread(bars, at, VOLATILITY_SHORT);
    let longer = spread(bars, at, LOOKBACK);

    if recent > 0.0 && longer > 0.0 {
        row[VOLATILITY] = (recent / longer).ln();
    }

    // The share of the recent movement that was upward, from -0.5 (all
    // down) to 0.5 (all up).
    let (mut up, mut down) = (0.0, 0.0);

    for step in steps(bars, at, OSCILLATOR_OVER) {
        if step > 0.0 {
            up += step;
        } else {
            down -= step;
        }
    }

    if up + down > 0.0 {
        row[OSCILLATOR] = up / (up + down) - 0.5;
    }

    let usual = spread(bars, at, MEDIUM);

    if usual > 0.0 {
        let window = &bars[(at + 1).saturating_sub(MEDIUM)..=at];
        let mean = window.iter().map(|bar| bar.close).sum::<f64>() / window.len() as f64;

        row[DISTANCE] = (bar.close / mean).ln() / usual;
        row[RETURN_IN_VOLATILITY] = row[RETURN] / usual;
    }

    row
}

/// The numbers for every bar.
pub fn table(bars: &[Bar]) -> Vec<Row> {
    (0..bars.len()).map(|at| row(bars, at)).collect()
}

/// The mean and spread of each of a set of numbers over the bars a model
/// was trained on, used to put every later value on the same scale.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scaler {
    pub mean: Vec<f64>,
    pub spread: Vec<f64>,
}

impl Scaler {
    /// Measures `rows`, which must be the training part only. Each row
    /// has `width` numbers.
    pub fn fit<'a>(rows: impl Iterator<Item = &'a [f64]>, width: usize) -> Self {
        let mut mean = vec![0.0; width];
        let mut squares = vec![0.0; width];
        let mut seen = 0.0;

        for row in rows {
            for (at, value) in row.iter().enumerate() {
                mean[at] += value;
                squares[at] += value * value;
            }

            seen += 1.0;
        }

        let seen = f64::max(seen, 1.0);

        for (mean, squares) in mean.iter_mut().zip(&mut squares) {
            *mean /= seen;
            *squares = (*squares / seen - *mean * *mean).max(0.0).sqrt();
        }

        Self {
            mean,
            spread: squares,
        }
    }

    /// `value`, the `at`-th number of a row, in spreads from the mean.
    /// A number that never varied in training comes out as 0.
    pub fn scaled(&self, at: usize, value: f64) -> f32 {
        if self.spread[at] < 1e-9 {
            return 0.0;
        }

        ((value - self.mean[at]) / self.spread[at]).clamp(-FURTHEST, FURTHEST) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synthetic;

    #[test]
    fn a_later_bar_never_changes_an_earlier_row() {
        let bars = synthetic::random_walk(160, 5);
        let rows = table(&bars);

        for changed in [1, 37, 80, 159] {
            let mut other = bars.clone();

            other[changed].open *= 1.3;
            other[changed].high *= 1.9;
            other[changed].low *= 0.4;
            other[changed].close *= 1.2;
            other[changed].volume *= 11.0;

            let other_rows = table(&other);

            assert_eq!(rows[..changed], other_rows[..changed], "bar {changed}");
            assert_ne!(rows[changed], other_rows[changed], "bar {changed}");
        }
    }

    #[test]
    fn a_row_is_the_same_from_the_last_bars_as_from_all_of_them() {
        let bars = synthetic::patterned(300, 2);
        let rows = table(&bars);

        for at in [KEPT - 1, KEPT, 150, 299] {
            let tail = &bars[at + 1 - KEPT..=at];

            assert_eq!(row(tail, KEPT - 1), rows[at], "bar {at}");
        }
    }

    #[test]
    fn rows_do_not_depend_on_the_price_level_or_the_size_of_volume() {
        let bars = synthetic::random_walk(120, 9);

        let scaled: Vec<Bar> = bars
            .iter()
            .map(|bar| Bar {
                time_ms: bar.time_ms,
                open: bar.open * 8.0,
                high: bar.high * 8.0,
                low: bar.low * 8.0,
                close: bar.close * 8.0,
                volume: bar.volume * 0.25,
            })
            .collect();

        for (left, right) in table(&bars).iter().zip(table(&scaled)) {
            for (left, right) in left.iter().zip(right) {
                assert!((left - right).abs() < 1e-9, "{left} {right}");
            }
        }
    }

    #[test]
    fn numbers_are_what_they_say() {
        let bar = |day: i64, open: f64, high: f64, low: f64, close: f64, volume: f64| Bar {
            time_ms: day * 86_400_000,
            open,
            high,
            low,
            close,
            volume,
        };

        let bars = [
            bar(0, 100.0, 100.0, 100.0, 100.0, 10.0),
            bar(1, 100.0, 110.0, 100.0, 110.0, 10.0),
            bar(2, 121.0, 125.0, 115.0, 120.0, 20.0),
        ];

        let last = row(&bars, 2);

        assert!((last[RETURN] - (120.0f64 / 110.0).ln()).abs() < 1e-12);
        assert!((last[RANGE] - 10.0 / 120.0).abs() < 1e-12);
        assert!(
            (last[POSITION] - 0.0).abs() < 1e-12,
            "the close is mid-range"
        );
        assert!((last[VOLUME] - 2.0f64.ln()).abs() < 1e-12);
        assert!((last[GAP] - (121.0f64 / 110.0).ln()).abs() < 1e-12);
        assert!((last[RETURN_5] - 1.2f64.ln()).abs() < 1e-12);
        assert!((last[OSCILLATOR] - 0.5).abs() < 1e-12, "every move was up");

        let first = row(&bars, 0);

        assert_eq!(first, [0.0; FEATURES]);
        assert!(table(&bars).iter().flatten().all(|value| value.is_finite()));
    }

    #[test]
    fn a_scaler_uses_only_the_rows_it_was_given() {
        let rows = [[1.0, 5.0], [3.0, 5.0], [100.0, 9.0]];

        let scaler = Scaler::fit(rows[..2].iter().map(|row| row.as_slice()), 2);

        assert_eq!(scaler.mean, [2.0, 5.0]);
        assert_eq!(scaler.spread, [1.0, 0.0]);
        assert_eq!(scaler.scaled(0, 3.0), 1.0);
        assert_eq!(scaler.scaled(0, 100.0), 5.0, "cut off");
        assert_eq!(scaler.scaled(1, 9.0), 0.0, "never varied in training");
    }
}
