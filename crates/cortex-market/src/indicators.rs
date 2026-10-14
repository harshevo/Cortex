//! The usual numbers traders read off a chart, worked out from the bars:
//! RSI, VWAP, two moving averages, the average true range, and the
//! levels where the price turned before.

use cortex_liquid::Bar;
use serde::{Deserialize, Serialize};

use crate::clock;

pub const RSI_OVER: usize = 14;
pub const ATR_OVER: usize = 14;
pub const FAST: usize = 9;
pub const SLOW: usize = 21;

/// Bars a daily VWAP is taken over, as there is no session to reset at.
const DAILY_VWAP_OVER: usize = 20;

/// One value per bar of each, absent until there are enough bars.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Indicators {
    pub rsi: Vec<Option<f64>>,
    pub vwap: Vec<Option<f64>>,
    pub ema_fast: Vec<Option<f64>>,
    pub ema_slow: Vec<Option<f64>>,
    pub atr: Vec<Option<f64>>,
}

impl Indicators {
    /// Works them all out. `intraday` bars reset the VWAP each day in
    /// India; daily bars take it over the last 20.
    pub fn of(bars: &[Bar], intraday: bool) -> Self {
        Self {
            rsi: rsi(bars, RSI_OVER),
            vwap: if intraday {
                session_vwap(bars)
            } else {
                rolling_vwap(bars, DAILY_VWAP_OVER)
            },
            ema_fast: ema(bars, FAST),
            ema_slow: ema(bars, SLOW),
            atr: atr(bars, ATR_OVER),
        }
    }

    pub fn last(values: &[Option<f64>]) -> Option<f64> {
        values.last().copied().flatten()
    }
}

/// Wilder's relative strength index.
pub fn rsi(bars: &[Bar], over: usize) -> Vec<Option<f64>> {
    let mut out = vec![None; bars.len()];

    if bars.len() <= over {
        return out;
    }

    let (mut gain, mut loss) = (0.0, 0.0);

    for at in 1..=over {
        let change = bars[at].close - bars[at - 1].close;

        if change > 0.0 {
            gain += change;
        } else {
            loss -= change;
        }
    }

    gain /= over as f64;
    loss /= over as f64;

    let value = |gain: f64, loss: f64| {
        if loss == 0.0 {
            if gain == 0.0 { 50.0 } else { 100.0 }
        } else {
            100.0 - 100.0 / (1.0 + gain / loss)
        }
    };

    out[over] = Some(value(gain, loss));

    for at in over + 1..bars.len() {
        let change = bars[at].close - bars[at - 1].close;

        gain = (gain * (over - 1) as f64 + change.max(0.0)) / over as f64;
        loss = (loss * (over - 1) as f64 + (-change).max(0.0)) / over as f64;
        out[at] = Some(value(gain, loss));
    }

    out
}

fn typical(bar: &Bar) -> f64 {
    (bar.high + bar.low + bar.close) / 3.0
}

/// The volume-weighted average price since the day's first bar. Bars
/// without volume count equally.
pub fn session_vwap(bars: &[Bar]) -> Vec<Option<f64>> {
    let mut out = Vec::with_capacity(bars.len());
    let mut day = i64::MIN;
    let (mut weighted, mut weight) = (0.0, 0.0);

    for bar in bars {
        let today = clock::ist_day(bar.time_ms);

        if today != day {
            day = today;
            weighted = 0.0;
            weight = 0.0;
        }

        let volume = if bar.volume > 0.0 { bar.volume } else { 1.0 };

        weighted += typical(bar) * volume;
        weight += volume;
        out.push(Some(weighted / weight));
    }

    out
}

pub fn rolling_vwap(bars: &[Bar], over: usize) -> Vec<Option<f64>> {
    (0..bars.len())
        .map(|at| {
            let window = &bars[(at + 1).saturating_sub(over)..=at];
            let (mut weighted, mut weight) = (0.0, 0.0);

            for bar in window {
                let volume = if bar.volume > 0.0 { bar.volume } else { 1.0 };

                weighted += typical(bar) * volume;
                weight += volume;
            }

            Some(weighted / weight)
        })
        .collect()
}

/// Exponential moving average of the close, seeded with the simple mean.
pub fn ema(bars: &[Bar], over: usize) -> Vec<Option<f64>> {
    let mut out = vec![None; bars.len()];

    if bars.len() < over || over == 0 {
        return out;
    }

    let mut value = bars[..over].iter().map(|bar| bar.close).sum::<f64>() / over as f64;
    let keep = 2.0 / (over as f64 + 1.0);

    out[over - 1] = Some(value);

    for at in over..bars.len() {
        value += keep * (bars[at].close - value);
        out[at] = Some(value);
    }

    out
}

/// Wilder's average true range.
pub fn atr(bars: &[Bar], over: usize) -> Vec<Option<f64>> {
    let mut out = vec![None; bars.len()];

    if bars.len() <= over {
        return out;
    }

    let range = |at: usize| {
        let bar = &bars[at];
        let before = bars[at - 1].close;

        (bar.high - bar.low)
            .max((bar.high - before).abs())
            .max((bar.low - before).abs())
    };

    let mut value = (1..=over).map(range).sum::<f64>() / over as f64;

    out[over] = Some(value);

    for at in over + 1..bars.len() {
        value = (value * (over - 1) as f64 + range(at)) / over as f64;
        out[at] = Some(value);
    }

    out
}

/// Where the price turned before, nearest the latest close.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Levels {
    /// Below the latest close, nearest first.
    pub supports: Vec<f64>,

    /// Above it, nearest first.
    pub resistances: Vec<f64>,

    /// The previous day's high, low and close, for intraday bars.
    pub previous_day: Option<(f64, f64, f64)>,

    /// Today's high and low so far, for intraday bars.
    pub today: Option<(f64, f64)>,
}

impl Levels {
    pub fn support(&self) -> Option<f64> {
        self.supports.first().copied()
    }

    pub fn resistance(&self) -> Option<f64> {
        self.resistances.first().copied()
    }
}

/// Swing highs and lows over the last `over` bars (a bar higher or lower
/// than the `side` bars either side of it), the day's and the previous
/// day's extremes, merged where they lie within a fifth of an average
/// range of each other.
pub fn levels(bars: &[Bar], intraday: bool, over: usize, side: usize) -> Levels {
    let Some(last) = bars.last() else {
        return Levels::default();
    };

    let window = &bars[bars.len().saturating_sub(over)..];
    let price = last.close;

    let mut found = Vec::new();

    for at in side..window.len().saturating_sub(side) {
        let around = &window[at - side..=at + side];
        let bar = &window[at];

        if around.iter().all(|other| other.high <= bar.high) {
            found.push(bar.high);
        }

        if around.iter().all(|other| other.low >= bar.low) {
            found.push(bar.low);
        }
    }

    let mut levels = Levels::default();

    if intraday {
        let today = clock::ist_day(last.time_ms);
        let of_day = |day: i64| {
            bars.iter()
                .filter(move |bar| clock::ist_day(bar.time_ms) == day)
        };

        let high = of_day(today).map(|bar| bar.high).fold(f64::MIN, f64::max);
        let low = of_day(today).map(|bar| bar.low).fold(f64::MAX, f64::min);

        levels.today = Some((high, low));

        if let Some(yesterday) = bars
            .iter()
            .rev()
            .map(|bar| clock::ist_day(bar.time_ms))
            .find(|day| *day < today)
        {
            let high = of_day(yesterday)
                .map(|bar| bar.high)
                .fold(f64::MIN, f64::max);
            let low = of_day(yesterday)
                .map(|bar| bar.low)
                .fold(f64::MAX, f64::min);
            let close = of_day(yesterday).next_back().map_or(price, |bar| bar.close);

            levels.previous_day = Some((high, low, close));
            found.extend([high, low, close]);
        }

        found.extend([high, low]);
    }

    // The window's own extremes are always levels.
    found.push(window.iter().map(|bar| bar.high).fold(f64::MIN, f64::max));
    found.push(window.iter().map(|bar| bar.low).fold(f64::MAX, f64::min));

    let atr = Indicators::last(&atr(bars, ATR_OVER)).unwrap_or_else(|| {
        window.iter().map(|bar| bar.high - bar.low).sum::<f64>() / window.len() as f64
    });
    let close_enough = (atr * 0.2).max(price * 1e-4);

    let mut above: Vec<f64> = found
        .iter()
        .copied()
        .filter(|level| *level > price)
        .collect();
    let mut below: Vec<f64> = found
        .iter()
        .copied()
        .filter(|level| *level < price)
        .collect();

    above.sort_by(f64::total_cmp);
    below.sort_by(|left, right| right.total_cmp(left));

    levels.resistances = merge(above, close_enough);
    levels.supports = merge(below, close_enough);

    levels
}

/// Keeps the first of each run of levels within `close_enough`.
fn merge(sorted: Vec<f64>, close_enough: f64) -> Vec<f64> {
    let mut kept: Vec<f64> = Vec::new();

    for level in sorted {
        if kept
            .last()
            .is_none_or(|last| (level - last).abs() > close_enough)
        {
            kept.push(level);
        }
    }

    kept.truncate(4);
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(at: i64, close: f64) -> Bar {
        Bar {
            time_ms: at * 300_000,
            open: close,
            high: close + 1.0,
            low: close - 1.0,
            close,
            volume: 100.0,
        }
    }

    #[test]
    fn rsi_is_100_on_only_rises_and_near_50_when_they_even_out() {
        let rising: Vec<Bar> = (0..30).map(|at| bar(at, 100.0 + at as f64)).collect();

        assert_eq!(Indicators::last(&rsi(&rising, 14)), Some(100.0));
        assert_eq!(rsi(&rising, 14)[13], None);

        let even: Vec<Bar> = (0..40)
            .map(|at| bar(at, if at % 2 == 0 { 100.0 } else { 101.0 }))
            .collect();

        let value = Indicators::last(&rsi(&even, 14)).unwrap();

        assert!((value - 50.0).abs() < 5.0, "{value}");
    }

    #[test]
    fn vwap_weights_by_volume_and_starts_again_each_day() {
        let mut bars = vec![bar(0, 100.0), bar(1, 110.0)];

        bars[1].volume = 300.0;

        let vwap = session_vwap(&bars);

        assert!((vwap[1].unwrap() - 107.5).abs() < 1e-9);

        let mut next_day = bar(0, 50.0);

        next_day.time_ms += clock::DAY_MS;
        bars.push(next_day);

        assert_eq!(session_vwap(&bars)[2], Some(50.0));
    }

    #[test]
    fn ema_follows_a_step() {
        let bars: Vec<Bar> = (0..40)
            .map(|at| bar(at, if at < 20 { 100.0 } else { 110.0 }))
            .collect();

        let fast = Indicators::last(&ema(&bars, 9)).unwrap();
        let slow = Indicators::last(&ema(&bars, 21)).unwrap();

        assert!(fast > slow && fast < 110.0 && slow > 100.0);
    }

    #[test]
    fn levels_lie_either_side_of_the_price() {
        let closes = [
            100.0, 104.0, 108.0, 104.0, 100.0, 96.0, 92.0, 96.0, 100.0, 103.0,
        ];
        let bars: Vec<Bar> = closes
            .iter()
            .enumerate()
            .map(|(at, close)| bar(at as i64, *close))
            .collect();

        let levels = levels(&bars, false, 100, 2);

        assert_eq!(levels.resistance(), Some(109.0));
        assert_eq!(levels.support(), Some(91.0));
    }
}
