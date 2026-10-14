//! Price bars read with the liquid and the dendritic network, for the
//! phones' Trading screen.
//!
//! The work is `cortex-liquid`'s; this hands it over in the plain shapes
//! Swift and Kotlin take. Nothing here is advice: the screen is given the
//! forecast, how such forecasts did on bars held out from training, and
//! whether that is any better than guessing.

use std::sync::Arc;

use cortex_liquid::{Desk, Direction, Edge, Predictor, Settings, Stage, Status};

/// Told when there is something new to fetch. Called from a background
/// thread.
#[uniffi::export(with_foreign)]
pub trait TradingListener: Send + Sync {
    fn changed(&self);
}

/// How far along the analysis is.
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum TradingStatus {
    /// Nothing has been loaded.
    Empty,

    /// There are bars and no analysis of them yet.
    Loaded,

    Working { stage: String, fraction: f32 },
    Done,
    Failed { reason: String },
}

/// The bars that are loaded.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TradingSeries {
    pub status: TradingStatus,

    /// The file's name.
    pub name: String,

    pub bars: u32,

    /// Milliseconds since 1970 of the first and the last bar.
    pub first_ms: i64,
    pub last_ms: i64,

    /// What had to be assumed or left out when the file was read.
    pub notes: Vec<String>,

    /// Changes whenever anything fetched from here would.
    pub revision: u64,
}

/// Several bars drawn as one candle.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct CandleView {
    /// The first bar in it, and how many.
    pub first: u32,
    pub count: u32,

    /// Milliseconds since 1970 of its first bar.
    pub time_ms: i64,

    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,

    /// The share of its scored bars whose direction was called right.
    pub right: Option<f32>,

    /// The regime of its middle bar, as a position in the report's
    /// `regimes`.
    pub regime: Option<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum EdgeView {
    /// Not told apart from guessing. The usual, honest answer.
    None,
    Weak,
    Present,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum DirectionView {
    Up,
    Down,
    Flat,
}

/// How one predictor did on the held-out bars.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ScoreView {
    pub name: String,

    /// One of the networks, as opposed to a baseline.
    pub learned: bool,

    /// The best baseline: what the networks have to beat.
    pub to_beat: bool,

    /// The share of directions called right, with the range the true
    /// share is likely in. `None` when it called none.
    pub accuracy: Option<f64>,
    pub accuracy_low: Option<f64>,
    pub accuracy_high: Option<f64>,

    /// Bars a direction was called on.
    pub scored: u32,
}

/// Something expected or imagined, with how sure.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ExpectedView {
    pub text: String,
    pub confidence: f32,
}

/// Everything the analysis found.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TradingReport {
    pub edge: EdgeView,

    /// The verdict in a sentence or two.
    pub verdict: String,

    /// Bars ahead that are forecast.
    pub horizon: u32,

    pub direction: DirectionView,

    /// The expected return over the horizon, as a share (0.01 is 1%).
    pub expected_return: f64,

    /// How sure the model is of the direction, from 0.5 to 1.
    pub confidence: f32,

    /// What the dendritic network sees now, the regime it is in, what it
    /// expects of the next bar and what it imagines after that.
    pub now: Vec<String>,
    pub regime: String,
    pub expects: Vec<ExpectedView>,
    pub imagines: Vec<ExpectedView>,

    pub scores: Vec<ScoreView>,

    /// The names of the regimes a candle's `regime` points into.
    pub regimes: Vec<String>,

    /// Bars used for training. Those after them were held out.
    pub train_bars: u32,

    /// The dendritic network after reading the bars.
    pub neurons: u32,
    pub branches: u32,
    pub synapses: u32,

    /// The share of each bar it expected, and the same for guessing the
    /// commonest.
    pub expected_right: f32,
    pub baseline_right: f32,

    /// The loss after each pass of training, on what was trained on and
    /// on bars held back from it, for the two together.
    pub loss_fitted: Vec<f32>,
    pub loss_checked: Vec<f32>,

    pub notes: Vec<String>,

    /// How long the analysis took.
    pub total_ms: f64,
}

/// One series being looked at.
#[derive(uniffi::Object)]
pub struct Trading {
    desk: Desk,
    listener: Arc<dyn TradingListener>,
}

#[uniffi::export]
impl Trading {
    #[uniffi::constructor]
    pub fn new(listener: Arc<dyn TradingListener>) -> Arc<Self> {
        Arc::new(Self {
            desk: Desk::new(),
            listener,
        })
    }

    /// Reads a CSV file's text as the bars to look at. Returns why it
    /// could not be read, when it could not; what was loaded before is
    /// then still there.
    pub fn load_csv(&self, name: String, text: String) -> Option<String> {
        self.desk
            .load_csv(&name, &text)
            .err()
            .map(|error| error.to_string())
    }

    /// Analyses the loaded bars in the background, forecasting `horizon`
    /// bars ahead. The listener is told as it goes and when it is done.
    pub fn analyse(&self, horizon: u32) {
        let listener = self.listener.clone();

        let settings = Settings {
            horizon: horizon.max(1) as usize,
            ..Settings::default()
        };

        self.desk.analyse(settings, move || listener.changed());
    }

    pub fn series(&self) -> TradingSeries {
        let bars = self.desk.bars();

        TradingSeries {
            status: match self.desk.status() {
                Status::Empty => TradingStatus::Empty,
                Status::Loaded => TradingStatus::Loaded,

                Status::Working { stage, fraction } => TradingStatus::Working {
                    stage: stage_name(stage).to_string(),
                    fraction,
                },

                Status::Done => TradingStatus::Done,
                Status::Failed(reason) => TradingStatus::Failed { reason },
            },
            name: self.desk.name(),
            bars: bars.len() as u32,
            first_ms: bars.first().map_or(0, |bar| bar.time_ms),
            last_ms: bars.last().map_or(0, |bar| bar.time_ms),
            notes: self.desk.notes(),
            revision: self.desk.revision(),
        }
    }

    /// The bars from `first` on, `count` of them, as at most `most`
    /// candles.
    pub fn candles(&self, first: u32, count: u32, most: u32) -> Vec<CandleView> {
        let bars = self.desk.bars();

        self.desk
            .candles(first as usize, count as usize, most as usize)
            .into_iter()
            .map(|candle| CandleView {
                first: candle.first as u32,
                count: candle.count as u32,
                time_ms: bars.get(candle.first).map_or(0, |bar| bar.time_ms),
                open: candle.open,
                high: candle.high,
                low: candle.low,
                close: candle.close,
                right: candle.right,
                regime: candle.regime,
            })
            .collect()
    }

    /// What the analysis found, once there is one.
    pub fn report(&self) -> Option<TradingReport> {
        let analysis = self.desk.analysis()?;

        let forecast = &analysis.forecast;
        let account = &forecast.dendritic;

        Some(TradingReport {
            edge: match analysis.verdict.edge {
                Edge::None => EdgeView::None,
                Edge::Weak => EdgeView::Weak,
                Edge::Present => EdgeView::Present,
            },
            verdict: analysis.verdict.text.clone(),
            horizon: forecast.horizon as u32,
            direction: match forecast.direction {
                Direction::Up => DirectionView::Up,
                Direction::Down => DirectionView::Down,
                Direction::Flat => DirectionView::Flat,
            },
            expected_return: forecast.expected_return,
            confidence: forecast.confidence,
            now: account.now.clone(),
            regime: account.regime.clone(),
            expects: account
                .expectations
                .iter()
                .map(|expectation| ExpectedView {
                    text: expectation.label.clone(),
                    confidence: expectation.confidence,
                })
                .collect(),
            imagines: account
                .imagined
                .iter()
                .map(|step| ExpectedView {
                    text: step.labels.join(", "),
                    confidence: step.confidence,
                })
                .collect(),
            scores: analysis
                .scores
                .iter()
                .map(|score| ScoreView {
                    name: score.name.clone(),
                    learned: matches!(
                        score.predictor,
                        Predictor::Liquid | Predictor::Dendritic | Predictor::Combined
                    ),
                    to_beat: score.predictor == analysis.verdict.best_baseline,
                    accuracy: score.accuracy,
                    accuracy_low: score.accuracy_low,
                    accuracy_high: score.accuracy_high,
                    scored: score.scored as u32,
                })
                .collect(),
            regimes: analysis.regime_names.clone(),
            train_bars: analysis.train_bars as u32,
            neurons: analysis.growth.neurons as u32,
            branches: analysis.growth.segments as u32,
            synapses: analysis.growth.synapses as u32,
            expected_right: analysis.growth.expected_right,
            baseline_right: analysis.growth.baseline_right,
            loss_fitted: analysis.combined_losses.iter().map(|loss| loss.fit).collect(),
            loss_checked: analysis.combined_losses.iter().map(|loss| loss.check).collect(),
            notes: analysis.notes.clone(),
            total_ms: analysis.timings.total_ms,
        })
    }
}

fn stage_name(stage: Stage) -> &'static str {
    match stage {
        Stage::Features => "Describing the bars",
        Stage::Dendritic => "The dendritic network is reading them",
        Stage::Liquid => "Training the liquid network",
        Stage::Combined => "Training the two together",
        Stage::Scoring => "Scoring on held-out bars",
        Stage::Done => "Done",
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{Sender, channel};
    use std::sync::Mutex;
    use std::time::Duration;

    use super::*;

    struct Tells(Mutex<Sender<()>>);

    impl TradingListener for Tells {
        fn changed(&self) {
            let _ = self.0.lock().unwrap().send(());
        }
    }

    /// A price that goes up two days and down one, with a little drift.
    fn file(rows: usize) -> String {
        let mut text = String::from("date,open,high,low,close,volume\n");
        let mut price = 100.0_f64;

        for row in 0..rows {
            let open = price;

            price *= if row % 3 == 2 { 0.99 } else { 1.006 };

            text.push_str(&format!(
                "{},{open:.4},{:.4},{:.4},{price:.4},{}\n",
                1_600_000_000 + row * 86_400,
                open.max(price) * 1.001,
                open.min(price) * 0.999,
                1_000 + row % 7
            ));
        }

        text
    }

    #[test]
    fn a_file_is_loaded_analysed_and_reported_in_plain_shapes() {
        let (tell, told) = channel();

        let trading = Trading::new(Arc::new(Tells(Mutex::new(tell))));

        assert_eq!(trading.series().status, TradingStatus::Empty);
        assert_eq!(trading.report(), None);

        assert_eq!(trading.load_csv("made-up.csv".into(), file(500)), None);

        let series = trading.series();

        assert_eq!(series.status, TradingStatus::Loaded);
        assert_eq!(series.bars, 500);
        assert!(series.last_ms > series.first_ms);

        trading.analyse(1);

        loop {
            told.recv_timeout(Duration::from_secs(120))
                .expect("the analysis never finished");

            match trading.series().status {
                TradingStatus::Working { .. } => {}
                status => {
                    assert_eq!(status, TradingStatus::Done);

                    break;
                }
            }
        }

        let report = trading.report().expect("there is a report");

        assert_eq!(report.horizon, 1);
        assert_eq!(report.scores.len(), 6);
        assert!(report.scores.iter().filter(|score| score.to_beat).count() == 1);
        assert!(report.neurons > 0 && report.branches > 0);
        assert_eq!(report.loss_fitted.len(), report.loss_checked.len());

        let candles = trading.candles(0, 500, 50);

        assert_eq!(candles.len(), 50);
        assert!(candles.iter().all(|candle| candle.regime.is_some()));
        assert!(candles[1].time_ms > candles[0].time_ms);
    }

    #[test]
    fn a_file_that_cannot_be_read_says_why() {
        let (tell, _told) = channel();

        let trading = Trading::new(Arc::new(Tells(Mutex::new(tell))));

        let problem = trading
            .load_csv("bad.csv".into(), "name,colour\nx,red\n".into())
            .expect("it cannot be read");

        assert!(!problem.is_empty());
        assert_eq!(trading.series().status, TradingStatus::Empty);
    }
}
