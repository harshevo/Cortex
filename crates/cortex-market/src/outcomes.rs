//! A record of forecasts made on live bars and how each came out: when a
//! bar closes, every network's forecast for 1, 5 and 10 bars ahead is
//! written down, and when those bars have closed it is marked right or
//! wrong. Kept in a file, so it grows across runs.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use cortex_liquid::Bar;
use serde::{Deserialize, Serialize};

/// Bars ahead that are written down.
pub const RECORDED_STEPS: [usize; 3] = [1, 5, 10];

/// The most kept; the oldest go first.
const KEPT: usize = 20_000;

/// One forecast, and what came of it once known.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Prediction {
    /// The instrument's key and the candle size, such as `NSE_EQ:2885` and
    /// `5m`.
    pub instrument: String,
    pub symbol: String,
    pub interval: String,

    /// The network's short name: `LNN`, `DANN`, `SNN`, `ENS` or `LLM`.
    pub source: String,

    /// The close of the bar it was made at, and that bar's time.
    pub made_ms: i64,
    pub from: f64,

    pub step: usize,

    /// When the bar it is about begins.
    pub due_ms: i64,

    pub close: f64,

    /// The 80% band around the close, where the network has one.
    pub low: Option<f64>,
    pub high: Option<f64>,

    pub outcome: Option<Outcome>,
}

impl Prediction {
    /// Up when it expects a higher close than the one it was made at.
    pub fn up(&self) -> Option<bool> {
        (self.close != self.from).then_some(self.close > self.from)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Outcome {
    pub actual: f64,

    /// Whether the direction was right. `None` when the price did not move
    /// or no direction was called.
    pub right: Option<bool>,

    /// Whether the close fell in the band.
    pub inside: Option<bool>,

    /// How far off the close was, in percent of the price.
    pub error_pct: f64,
}

/// How one network has done, at one number of bars ahead.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Tally {
    pub source: String,
    pub step: usize,
    pub made: usize,
    pub resolved: usize,

    /// Of the resolved, those with a direction to judge, and the right ones.
    pub judged: usize,
    pub right: usize,

    pub inside: usize,
    pub banded: usize,
    pub mean_error_pct: f64,
}

impl Tally {
    pub fn accuracy(&self) -> Option<f64> {
        (self.judged > 0).then(|| self.right as f64 / self.judged as f64)
    }

    pub fn inside_rate(&self) -> Option<f64> {
        (self.banded > 0).then(|| self.inside as f64 / self.banded as f64)
    }
}

#[derive(Debug, Default)]
pub struct Ledger {
    file: Option<PathBuf>,
    predictions: Vec<Prediction>,
    keys: HashSet<(String, String, String, i64, usize)>,
}

fn key(prediction: &Prediction) -> (String, String, String, i64, usize) {
    (
        prediction.instrument.clone(),
        prediction.interval.clone(),
        prediction.source.clone(),
        prediction.made_ms,
        prediction.step,
    )
}

impl Ledger {
    /// The ledger kept in `file`, empty if there is none or it cannot be
    /// read.
    pub fn open(file: &Path) -> Self {
        let predictions: Vec<Prediction> = std::fs::read_to_string(file)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();

        let keys = predictions.iter().map(key).collect();

        Self {
            file: Some(file.to_path_buf()),
            predictions,
            keys,
        }
    }

    /// One that is never written.
    pub fn in_memory() -> Self {
        Self::default()
    }

    pub fn save(&self) {
        let Some(file) = &self.file else {
            return;
        };

        if let Some(parent) = file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        if let Ok(text) = serde_json::to_string(&self.predictions) {
            let temporary = file.with_extension("json.tmp");

            if std::fs::write(&temporary, text).is_ok() {
                let _ = std::fs::rename(&temporary, file);
            }
        }
    }

    /// Writes a forecast down, unless the same one already is.
    pub fn record(&mut self, prediction: Prediction) -> bool {
        if !self.keys.insert(key(&prediction)) {
            return false;
        }

        self.predictions.push(prediction);

        if self.predictions.len() > KEPT {
            let gone = self.predictions.len() - KEPT;

            for old in self.predictions.drain(..gone) {
                self.keys.remove(&key(&old));
            }
        }

        true
    }

    /// Marks the forecasts for `instrument` whose bar has closed among
    /// `bars` (which must all be closed). Returns how many were marked.
    pub fn resolve(&mut self, instrument: &str, interval: &str, bars: &[Bar]) -> usize {
        let mut marked = 0;

        for prediction in &mut self.predictions {
            if prediction.outcome.is_some()
                || prediction.instrument != instrument
                || prediction.interval != interval
            {
                continue;
            }

            let Ok(made) = bars.binary_search_by_key(&prediction.made_ms, |bar| bar.time_ms) else {
                continue;
            };

            let Some(bar) = bars.get(made + prediction.step) else {
                continue;
            };

            let actual = bar.close;

            let right = prediction.up().and_then(|up| {
                (actual != prediction.from).then_some(up == (actual > prediction.from))
            });

            let inside = match (prediction.low, prediction.high) {
                (Some(low), Some(high)) => Some(actual >= low && actual <= high),
                _ => None,
            };

            prediction.due_ms = bar.time_ms;
            prediction.outcome = Some(Outcome {
                actual,
                right,
                inside,
                error_pct: ((prediction.close - actual) / actual * 100.0).abs(),
            });

            marked += 1;
        }

        marked
    }

    /// The latest `most` forecasts, newest first, of `instrument` or of
    /// all.
    pub fn latest(&self, instrument: Option<&str>, most: usize) -> Vec<Prediction> {
        self.predictions
            .iter()
            .rev()
            .filter(|prediction| instrument.is_none_or(|key| prediction.instrument == key))
            .take(most)
            .cloned()
            .collect()
    }

    /// How each network has done at each number of bars ahead.
    pub fn tallies(&self, instrument: Option<&str>) -> Vec<Tally> {
        let mut tallies: BTreeMap<(String, usize), Tally> = BTreeMap::new();
        let mut errors: BTreeMap<(String, usize), f64> = BTreeMap::new();

        for prediction in &self.predictions {
            if instrument.is_some_and(|key| prediction.instrument != key) {
                continue;
            }

            let id = (prediction.source.clone(), prediction.step);
            let tally = tallies.entry(id.clone()).or_insert_with(|| Tally {
                source: prediction.source.clone(),
                step: prediction.step,
                ..Tally::default()
            });

            tally.made += 1;

            let Some(outcome) = prediction.outcome else {
                continue;
            };

            tally.resolved += 1;
            *errors.entry(id).or_default() += outcome.error_pct;

            if let Some(right) = outcome.right {
                tally.judged += 1;
                tally.right += usize::from(right);
            }

            if let Some(inside) = outcome.inside {
                tally.banded += 1;
                tally.inside += usize::from(inside);
            }
        }

        tallies
            .into_iter()
            .map(|(id, mut tally)| {
                if tally.resolved > 0 {
                    tally.mean_error_pct =
                        errors.get(&id).copied().unwrap_or(0.0) / tally.resolved as f64;
                }

                tally
            })
            .collect()
    }

    pub fn len(&self) -> usize {
        self.predictions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.predictions.is_empty()
    }

    /// Forgets every forecast of `instrument`, or all.
    pub fn clear(&mut self, instrument: Option<&str>) {
        self.predictions
            .retain(|prediction| instrument.is_some_and(|key| prediction.instrument != key));
        self.keys = self.predictions.iter().map(key).collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(at: i64, close: f64) -> Bar {
        Bar {
            time_ms: at * 60_000,
            open: close,
            high: close,
            low: close,
            close,
            volume: 0.0,
        }
    }

    fn prediction(source: &str, step: usize, close: f64) -> Prediction {
        Prediction {
            instrument: "NSE_EQ:1".to_string(),
            symbol: "X".to_string(),
            interval: "1m".to_string(),
            source: source.to_string(),
            made_ms: 60_000,
            from: 100.0,
            step,
            due_ms: 0,
            close,
            low: Some(close - 1.0),
            high: Some(close + 1.0),
            outcome: None,
        }
    }

    #[test]
    fn forecasts_are_marked_when_their_bar_closes() {
        let mut ledger = Ledger::in_memory();

        assert!(ledger.record(prediction("LNN", 1, 101.0)));
        assert!(
            !ledger.record(prediction("LNN", 1, 101.0)),
            "the same one twice"
        );
        assert!(ledger.record(prediction("SNN", 1, 99.0)));
        assert!(ledger.record(prediction("LNN", 5, 103.0)));

        let bars = vec![bar(0, 99.0), bar(1, 100.0), bar(2, 100.5)];

        assert_eq!(ledger.resolve("NSE_EQ:1", "1m", &bars), 2);

        let tallies = ledger.tallies(None);
        let lnn = tallies
            .iter()
            .find(|t| t.source == "LNN" && t.step == 1)
            .unwrap();
        let snn = tallies
            .iter()
            .find(|t| t.source == "SNN" && t.step == 1)
            .unwrap();

        assert_eq!(lnn.accuracy(), Some(1.0));
        assert_eq!(lnn.inside_rate(), Some(1.0));
        assert_eq!(snn.accuracy(), Some(0.0));

        let five = tallies.iter().find(|t| t.step == 5).unwrap();

        assert_eq!((five.made, five.resolved), (1, 0));
    }

    #[test]
    fn the_ledger_is_kept_in_its_file() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("outcomes.json");

        let mut ledger = Ledger::open(&file);

        ledger.record(prediction("ENS", 10, 104.0));
        ledger.save();

        let again = Ledger::open(&file);

        assert_eq!(again.len(), 1);
        assert_eq!(again.latest(Some("NSE_EQ:1"), 5)[0].source, "ENS");
        assert!(again.latest(Some("other"), 5).is_empty());
    }
}
