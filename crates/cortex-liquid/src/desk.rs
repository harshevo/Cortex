//! One series being looked at: the bars that were loaded, the analysis
//! of them once there is one, and how far along it is.
//!
//! The analysis takes a second or so, so it runs on a thread of its own
//! and whoever shows it is told when there is something new to fetch.
//! Both apps keep a [`Desk`] and draw what it holds.

use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;

use crate::analysis::{Analysis, Model, Progress, Settings, Stage};
use crate::bars::{Bar, parse_csv};
use crate::error::Error;

/// How far along the desk is.
#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    /// Nothing has been loaded.
    Empty,

    /// There are bars and no analysis of them yet.
    Loaded,

    Working { stage: Stage, fraction: f32 },

    /// The analysis is there to fetch.
    Done,

    Failed(String),
}

#[derive(Default)]
struct Held {
    name: String,
    bars: Arc<Vec<Bar>>,

    /// What had to be assumed or left out when the file was read.
    notes: Vec<String>,

    status: Option<Status>,
    analysis: Option<Arc<Analysis>>,
    model: Option<Model>,

    /// Counts the loads, so that an analysis that finishes after other
    /// bars were loaded is known to be of the wrong ones.
    load: u64,

    /// Changes whenever anything a screen shows does.
    revision: u64,
}

#[derive(Clone, Default)]
pub struct Desk {
    held: Arc<Mutex<Held>>,
}

impl Desk {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads a CSV file's text as the bars to look at. What was loaded
    /// and analysed before is dropped, unless the text cannot be read:
    /// then nothing changes and the error says what was wrong with it.
    pub fn load_csv(&self, name: &str, text: &str) -> Result<usize, Error> {
        let parsed = parse_csv(text)?;

        let mut notes = parsed.notes;

        if parsed.skipped > 0 {
            notes.push(match &parsed.first_problem {
                Some(problem) => format!(
                    "{} rows could not be read and were left out. The first: {problem}",
                    parsed.skipped
                ),
                None => format!("{} rows could not be read and were left out.", parsed.skipped),
            });
        }

        if parsed.duplicates > 0 {
            notes.push(format!(
                "{} rows had the same time as a later row and were left out.",
                parsed.duplicates
            ));
        }

        let count = parsed.bars.len();

        self.load_bars(name, parsed.bars, notes);

        Ok(count)
    }

    /// Takes bars that are already in order as the ones to look at.
    pub fn load_bars(&self, name: &str, bars: Vec<Bar>, notes: Vec<String>) {
        let mut held = self.held();

        held.name = name.to_string();
        held.bars = Arc::new(bars);
        held.notes = notes;
        held.status = Some(Status::Loaded);
        held.analysis = None;
        held.model = None;
        held.load += 1;
        held.revision += 1;
    }

    /// Analyses the loaded bars on a thread of its own. `changed` is
    /// called, from that thread, whenever there is something new to
    /// fetch. Does nothing while an analysis is already running or when
    /// nothing is loaded.
    pub fn analyse(&self, settings: Settings, changed: impl Fn() + Send + Sync + 'static) {
        let (bars, load) = {
            let mut held = self.held();

            if held.bars.is_empty() || matches!(held.status, Some(Status::Working { .. })) {
                return;
            }

            held.status = Some(Status::Working {
                stage: Stage::Features,
                fraction: 0.0,
            });
            held.revision += 1;

            (held.bars.clone(), held.load)
        };

        let desk = self.clone();

        thread::spawn(move || {
            let progress = |progress: Progress| {
                let mut held = desk.held();

                if held.load != load {
                    return;
                }

                held.status = Some(Status::Working {
                    stage: progress.stage,
                    fraction: progress.fraction,
                });
                held.revision += 1;

                drop(held);

                changed();
            };

            let trained = Model::train(&bars, &settings, &progress);

            let mut held = desk.held();

            // Other bars were loaded meanwhile: this is not about them.
            if held.load != load {
                return;
            }

            match trained {
                Ok((model, analysis)) => {
                    held.model = Some(model);
                    held.analysis = Some(Arc::new(analysis));
                    held.status = Some(Status::Done);
                }

                Err(error) => held.status = Some(Status::Failed(error.to_string())),
            }

            held.revision += 1;

            drop(held);

            changed();
        });
    }

    pub fn status(&self) -> Status {
        self.held().status.clone().unwrap_or(Status::Empty)
    }

    /// What the bars were loaded as: a file's name.
    pub fn name(&self) -> String {
        self.held().name.clone()
    }

    pub fn bars(&self) -> Arc<Vec<Bar>> {
        self.held().bars.clone()
    }

    pub fn notes(&self) -> Vec<String> {
        self.held().notes.clone()
    }

    pub fn analysis(&self) -> Option<Arc<Analysis>> {
        self.held().analysis.clone()
    }

    /// The bars from `first` on, `count` of them, as at most `most`
    /// candles, each with how the forecasts over its bars went and its
    /// regime once there is an analysis.
    pub fn candles(&self, first: usize, count: usize, most: usize) -> Vec<Candle> {
        let held = self.held();

        candles(&held.bars, held.analysis.as_deref(), first, count, most)
    }

    /// The trained model as text, to keep.
    pub fn model_json(&self) -> Option<String> {
        self.held().model.as_ref().map(Model::to_json)
    }

    /// Changes whenever anything fetched from here would.
    pub fn revision(&self) -> u64 {
        self.held().revision
    }

    fn held(&self) -> MutexGuard<'_, Held> {
        self.held.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Several bars drawn as one candle: a chart has room for only so many.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Candle {
    /// The first bar in it, and how many.
    pub first: usize,
    pub count: usize,

    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,

    /// The share of its scored bars whose direction the two networks
    /// together called right. `None` where none of its bars was scored.
    pub right: Option<f32>,

    /// The regime of its middle bar, as a position in the analysis's
    /// `regime_names`.
    pub regime: Option<u8>,
}

/// The bars from `first` on, `count` of them, as at most `most` candles.
pub fn candles(
    bars: &[Bar],
    analysis: Option<&Analysis>,
    first: usize,
    count: usize,
    most: usize,
) -> Vec<Candle> {
    let first = first.min(bars.len());
    let end = first.saturating_add(count).min(bars.len());

    if first == end || most == 0 {
        return Vec::new();
    }

    let each = (end - first).div_ceil(most).max(1);

    bars[first..end]
        .chunks(each)
        .enumerate()
        .map(|(at, chunk)| {
            let first = first + at * each;

            Candle {
                first,
                count: chunk.len(),
                open: chunk[0].open,
                close: chunk[chunk.len() - 1].close,
                high: chunk.iter().map(|bar| bar.high).fold(f64::MIN, f64::max),
                low: chunk.iter().map(|bar| bar.low).fold(f64::MAX, f64::min),
                right: analysis.and_then(|analysis| called_right(analysis, first, chunk.len())),
                regime: analysis
                    .and_then(|analysis| analysis.regimes.get(first + chunk.len() / 2).copied()),
            }
        })
        .collect()
}

/// The share of the scored bars among `count` from `first` whose
/// direction was called right.
fn called_right(analysis: &Analysis, first: usize, count: usize) -> Option<f32> {
    let series = &analysis.series;

    let mut scored = 0;
    let mut right = 0;

    for bar in first..first + count {
        let Some(at) = bar.checked_sub(series.first_bar) else {
            continue;
        };

        let (Some(predicted), Some(Some(actual))) =
            (series.predicted_return.get(at), series.actual_return.get(at))
        else {
            continue;
        };

        // A bar that did not move has no direction to call.
        if *actual == 0.0 {
            continue;
        }

        scored += 1;

        if (*predicted > 0.0) == (*actual > 0.0) {
            right += 1;
        }
    }

    (scored > 0).then(|| right as f32 / scored as f32)
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;
    use crate::synthetic;

    fn quick() -> Settings {
        Settings {
            epochs: 3,
            units: 8,
            ..Settings::default()
        }
    }

    fn wait(desk: &Desk, told: &mpsc::Receiver<()>) -> Status {
        loop {
            told.recv_timeout(Duration::from_secs(60))
                .expect("the analysis never finished");

            match desk.status() {
                Status::Working { .. } => {}
                status => return status,
            }
        }
    }

    #[test]
    fn bars_are_loaded_analysed_and_the_result_fetched() {
        let desk = Desk::new();

        assert_eq!(desk.status(), Status::Empty);

        desk.load_bars("made up", synthetic::patterned(600, 3), Vec::new());

        assert_eq!(desk.status(), Status::Loaded);

        let (tell, told) = mpsc::channel();

        desk.analyse(quick(), move || {
            let _ = tell.send(());
        });

        assert_eq!(wait(&desk, &told), Status::Done);

        let analysis = desk.analysis().expect("there is an analysis");

        assert_eq!(analysis.bars, 600);
        assert!(desk.model_json().is_some());
    }

    #[test]
    fn a_file_that_cannot_be_read_changes_nothing() {
        let desk = Desk::new();

        desk.load_bars("made up", synthetic::random_walk(50, 1), Vec::new());

        let revision = desk.revision();

        assert!(desk.load_csv("bad.csv", "name,colour\nx,red\n").is_err());
        assert_eq!(desk.revision(), revision);
        assert_eq!(desk.name(), "made up");
    }

    #[test]
    fn too_few_bars_fail_with_the_reason() {
        let desk = Desk::new();

        desk.load_bars("short", synthetic::random_walk(20, 1), Vec::new());

        let (tell, told) = mpsc::channel();

        desk.analyse(quick(), move || {
            let _ = tell.send(());
        });

        match wait(&desk, &told) {
            Status::Failed(reason) => assert!(reason.contains("bars"), "{reason}"),
            status => panic!("{status:?}"),
        }
    }

    #[test]
    fn an_analysis_of_bars_since_replaced_is_dropped() {
        let desk = Desk::new();

        desk.load_bars("first", synthetic::patterned(600, 3), Vec::new());
        desk.analyse(quick(), || {});
        desk.load_bars("second", synthetic::random_walk(40, 2), Vec::new());

        std::thread::sleep(Duration::from_millis(1_500));

        assert_eq!(desk.status(), Status::Loaded);
        assert!(desk.analysis().is_none());
        assert_eq!(desk.name(), "second");
    }

    fn bar(at: usize, open: f64, high: f64, low: f64, close: f64) -> Bar {
        Bar {
            time_ms: at as i64 * 86_400_000,
            open,
            high,
            low,
            close,
            volume: 1.0,
        }
    }

    #[test]
    fn bars_that_do_not_fit_are_drawn_several_to_a_candle() {
        let bars: Vec<Bar> = (0..10)
            .map(|at| {
                let price = at as f64;

                bar(at, price, price + 2.0, price - 1.0, price + 1.0)
            })
            .collect();

        let drawn = candles(&bars, None, 0, 10, 3);

        assert_eq!(drawn.len(), 3);

        // Four bars in the first: it opens where the first did, closes
        // where the fourth did, and spans the highest and lowest of them.
        assert_eq!(
            drawn[0],
            Candle {
                first: 0,
                count: 4,
                open: 0.0,
                high: 5.0,
                low: -1.0,
                close: 4.0,
                right: None,
                regime: None,
            }
        );

        assert_eq!(drawn[2].first, 8);
        assert_eq!(drawn[2].count, 2);

        // Every bar is in exactly one candle.
        assert_eq!(drawn.iter().map(|candle| candle.count).sum::<usize>(), 10);
    }

    #[test]
    fn a_window_past_the_end_draws_what_there_is() {
        let bars: Vec<Bar> = (0..5).map(|at| bar(at, 1.0, 2.0, 0.5, 1.5)).collect();

        assert_eq!(candles(&bars, None, 3, 10, 100).len(), 2);
        assert!(candles(&bars, None, 9, 10, 100).is_empty());
        assert!(candles(&bars, None, 0, usize::MAX, 100).len() == 5);
        assert!(candles(&[], None, 0, 10, 100).is_empty());
    }

    #[test]
    fn candles_over_held_out_bars_say_how_the_calls_went() {
        let desk = Desk::new();

        desk.load_bars("made up", synthetic::patterned(600, 3), Vec::new());

        let (tell, told) = mpsc::channel();

        desk.analyse(quick(), move || {
            let _ = tell.send(());
        });

        assert_eq!(wait(&desk, &told), Status::Done);

        let analysis = desk.analysis().unwrap();
        let drawn = desk.candles(0, 600, 60);

        assert!(drawn.iter().all(|candle| candle.regime.is_some()));

        // Only bars the networks were scored on have calls to judge.
        for candle in &drawn {
            if candle.first + candle.count <= analysis.series.first_bar {
                assert_eq!(candle.right, None);
            }
        }

        assert!(drawn.iter().any(|candle| candle.right.is_some()));
    }

    #[test]
    fn rows_left_out_are_said() {
        let desk = Desk::new();

        let text = "date,close\n2024-01-01,10\nnot a date,11\n2024-01-03,12\n";

        assert_eq!(desk.load_csv("few.csv", text).unwrap(), 2);
        assert!(desk.notes().iter().any(|note| note.contains("1 rows could not be read")));
    }
}
