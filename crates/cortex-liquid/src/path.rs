//! The next few candles, from each network: open, high, low, close and
//! volume for each of the next [`STEPS`] bars, and how well each network
//! did at that on bars it was not trained on.
//!
//! Three networks read the same bars:
//!
//! - **Liquid** ([`crate::cfc`]): the closed-form continuous-time cell,
//!   told how much time passed between bars, with one output per number
//!   predicted.
//! - **Dendritic** ([`crate::regime`]): the dendritic network lives each
//!   bar as a few words; what it expects next, and the words for the bar
//!   now, are read out by a linear layer.
//! - **Spiking** ([`crate::spiking`]): a reservoir of leaky
//!   integrate-and-fire neurons, its recent firing read out by a linear
//!   layer.
//!
//! An **ensemble** averages the three, each weighted by how well it did on
//! the training bars held back for checking.
//!
//! Every target is relative to the close of the bar the forecast is made
//! at, so a model trained on one price level can be run on another:
//!
//! - the log return from that close to each later close,
//! - each later bar's upper and lower wick, as log ratios to its body,
//! - each later bar's opening gap, as a log ratio to the close before,
//! - each later bar's volume, as a log ratio to the last 20 bars' mean.
//!
//! What this is not. Prices are close to a random walk; on most series the
//! held-out accuracy of every network is near 50% and the error near that
//! of predicting no change. The metrics say so when it is so.

use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::bars::{self, Bar};
use crate::cfc::{Cfc, EpochLoss, Head, Lesson, Shape, Training};
use crate::features::{self, FEATURES, KEPT, Row, Scaler};
use crate::regime::{Account, Buckets, Codes, Growth, OUTLOOK, Regime};
use crate::ridge::Ridge;
use crate::spiking::{Activity, Reservoir};
use crate::stats;

/// Bars ahead forecast.
pub const STEPS: usize = 10;

/// Numbers forecast per bar ahead.
const PARTS: usize = 5;

const CLOSE: usize = 0;
const UPPER: usize = 1;
const LOWER: usize = 2;
const OPENING: usize = 3;
const VOLUME: usize = 4;

/// Numbers forecast in all.
pub const OUTPUTS: usize = STEPS * PARTS;

/// Bars ahead the metrics are given for.
pub const SCORED_STEPS: [usize; 3] = [1, 5, 10];

const WARMUP: usize = 20;
const FEWEST_FITTED: usize = 80;
const FEWEST_SCORED: usize = 20;

/// Share of the training samples held back to pick epochs, penalties and
/// the ensemble's weights.
const CHECKED: f64 = 0.15;

const SHORTEST_GAP: f64 = 0.05;
const LONGEST_GAP: f64 = 10.0;

/// Standardised targets are cut off this far out.
const FURTHEST: f32 = 5.0;

/// Numbers the dendritic read-out takes: its outlook, and the words for
/// the bar now, one-hot.
const DENDRITIC_INPUTS: usize = OUTLOOK + 5 + 3 + 3 + 3 + 3;

const PENALTIES: [f64; 5] = [0.1, 1.0, 10.0, 100.0, 1_000.0];

const Z_99: f64 = 2.576;

/// Which network a forecast is from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Net {
    Liquid,
    Dendritic,
    Spiking,
    Ensemble,
}

impl Net {
    /// The three networks that learn, in the order their outputs are kept.
    pub const LEARNED: [Net; 3] = [Net::Liquid, Net::Dendritic, Net::Spiking];

    pub const ALL: [Net; 4] = [Net::Liquid, Net::Dendritic, Net::Spiking, Net::Ensemble];

    pub fn name(self) -> &'static str {
        match self {
            Net::Liquid => "Liquid",
            Net::Dendritic => "Dendritic",
            Net::Spiking => "Spiking",
            Net::Ensemble => "Ensemble",
        }
    }

    /// The usual short name.
    pub fn short(self) -> &'static str {
        match self {
            Net::Liquid => "LNN",
            Net::Dendritic => "DANN",
            Net::Spiking => "SNN",
            Net::Ensemble => "ENS",
        }
    }

    fn slot(self) -> usize {
        match self {
            Net::Liquid => 0,
            Net::Dendritic => 1,
            Net::Spiking => 2,
            Net::Ensemble => 3,
        }
    }
}

/// How to train.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PathSettings {
    /// Units of state in the liquid network.
    pub units: usize,
    pub epochs: usize,
    pub window: usize,

    /// Neurons in the spiking reservoir.
    pub neurons: usize,

    /// Share of the bars, the latest, held out for the metrics.
    pub test_share: f32,
    pub seed: u64,
}

impl Default for PathSettings {
    fn default() -> Self {
        Self {
            units: 24,
            epochs: 24,
            window: 32,
            neurons: 192,
            test_share: 0.2,
            seed: 1,
        }
    }
}

impl PathSettings {
    /// The fewest bars these settings can be trained on.
    pub fn fewest_bars(&self) -> usize {
        let share = f64::from(self.test_share.clamp(0.05, 0.5));
        let training = (WARMUP + FEWEST_FITTED + STEPS) as f64 / (1.0 - share);
        let scoring = (FEWEST_SCORED + STEPS) as f64 / share;

        training.max(scoring).ceil() as usize + 1
    }
}

/// How a network did at one number of bars ahead, on held-out bars.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepScore {
    /// Bars ahead.
    pub step: usize,

    /// Held-out samples where a direction was called and the price moved.
    pub scored: usize,
    pub right: usize,
    pub accuracy: Option<f64>,

    /// The 99% interval for the accuracy.
    pub low: Option<f64>,
    pub high: Option<f64>,

    /// The chance of an accuracy this far from 50% from guessing.
    pub p_value: Option<f64>,

    /// Mean distance between the predicted and the actual close, in
    /// percent of the price.
    pub error_pct: f64,

    /// 1 minus its squared error over that of predicting no change. Above
    /// 0 it did better than no change.
    pub skill: f64,
}

/// Everything measured about one network.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetMetrics {
    pub net: Net,

    /// At each of [`SCORED_STEPS`].
    pub steps: Vec<StepScore>,

    /// Share of held-out next bars whose close fell inside the predicted
    /// 80% band. Near 0.8 when the band is honest.
    pub band_hit: Option<f64>,

    /// Mean distance between the predicted and the actual next volume,
    /// as a share of the actual. `None` without volume.
    pub volume_error: Option<f64>,

    /// Weights trained.
    pub parameters: usize,

    /// Neurons, where the network has neurons to count.
    pub neurons: usize,

    /// Its weight in the ensemble, from 0 to 1.
    pub weight: f32,

    pub train_ms: f64,

    /// Loss per epoch, for the liquid network.
    pub losses: Vec<EpochLoss>,

    /// Facts particular to the network, as name and value.
    pub facts: Vec<(String, String)>,
}

impl NetMetrics {
    pub fn at(&self, step: usize) -> Option<&StepScore> {
        self.steps.iter().find(|score| score.step == step)
    }
}

/// What training and the held-out bars showed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PathReport {
    pub bars: usize,
    pub train_bars: usize,

    /// Held-out samples whose next [`STEPS`] bars are known.
    pub scored: usize,

    /// One per network, ensemble last.
    pub metrics: Vec<NetMetrics>,

    /// Calling the direction the price went over the same number of bars
    /// just gone.
    pub baseline: Vec<StepScore>,

    /// How the dendritic network grew.
    pub growth: Growth,

    pub total_ms: f64,
}

impl PathReport {
    pub fn of(&self, net: Net) -> Option<&NetMetrics> {
        self.metrics.iter().find(|metrics| metrics.net == net)
    }
}

/// One bar ahead as forecast.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PredictedCandle {
    /// Bars ahead, from 1.
    pub step: usize,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,

    /// 0 where the bars have no volume.
    pub volume: f64,

    /// The range the close fell in four times in five on the training
    /// bars held back for checking: an 80% band, from the network's own
    /// errors.
    pub close_low: f64,
    pub close_high: f64,
}

/// The next [`STEPS`] bars as one network forecasts them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Path {
    pub net: Net,

    /// The bar the forecast is made at.
    pub after_time_ms: i64,
    pub from_close: f64,
    pub candles: Vec<PredictedCandle>,
}

impl Path {
    /// The forecast close `step` bars ahead.
    pub fn close(&self, step: usize) -> Option<f64> {
        self.candles
            .iter()
            .find(|candle| candle.step == step)
            .map(|candle| candle.close)
    }
}

/// Mean and spread of each target over the training samples.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct TargetScale {
    mean: Vec<f64>,
    spread: Vec<f64>,
}

impl TargetScale {
    fn value(&self, at: usize, standard: f32) -> f64 {
        f64::from(standard) * self.spread[at] + self.mean[at]
    }

    fn standard(&self, at: usize, value: f64) -> f32 {
        (((value - self.mean[at]) / self.spread[at]) as f32).clamp(-FURTHEST, FURTHEST)
    }
}

/// The three networks, trained, and where each is in the series.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Forecaster {
    settings: PathSettings,
    scaler: Scaler,
    scale: TargetScale,
    gap_ms: f64,

    cell: Cfc,
    cell_state: Vec<f32>,

    regime: Regime,
    dendritic: Ridge,

    reservoir: Reservoir,
    activity: Activity,
    spiking: Ridge,

    /// The ensemble's weight for each learned network.
    weights: [f32; 3],

    /// For each network (ensemble last) and bar ahead, the 10th and 90th
    /// percentile of its error in the log return to the close.
    bands: Vec<Vec<(f64, f64)>>,

    recent: Vec<Bar>,

    /// The latest standardised outputs of each learned network.
    latest: [Vec<f32>; 3],
}

/// The return over the bars just gone, or nothing at the start.
fn log_ratio(over: f64, under: f64) -> f64 {
    if over > 0.0 && under > 0.0 {
        (over / under).ln()
    } else {
        0.0
    }
}

/// The mean volume of the 20 bars up to `at`.
fn usual_volume(bars: &[Bar], at: usize) -> f64 {
    let window = &bars[(at + 1).saturating_sub(20)..=at];

    window.iter().map(|bar| bar.volume).sum::<f64>() / window.len() as f64
}

/// What the next [`STEPS`] bars after `at` were, as targets.
fn targets(bars: &[Bar], at: usize) -> Option<[f64; OUTPUTS]> {
    if at + STEPS >= bars.len() {
        return None;
    }

    let from = bars[at].close;
    let usual = usual_volume(bars, at);
    let mut out = [0.0; OUTPUTS];

    for step in 1..=STEPS {
        let bar = &bars[at + step];
        let before = &bars[at + step - 1];
        let k = step - 1;

        out[CLOSE * STEPS + k] = log_ratio(bar.close, from);
        out[UPPER * STEPS + k] = log_ratio(bar.high, bar.open.max(bar.close)).max(0.0);
        out[LOWER * STEPS + k] = log_ratio(bar.open.min(bar.close), bar.low).max(0.0);
        out[OPENING * STEPS + k] = log_ratio(bar.open, before.close);
        out[VOLUME * STEPS + k] = if usual > 0.0 && bar.volume > 0.0 {
            (bar.volume / usual).ln()
        } else {
            0.0
        };
    }

    Some(out)
}

fn gap(bars: &[Bar], at: usize, usual_ms: f64) -> f32 {
    if at == 0 {
        return 1.0;
    }

    let elapsed = (bars[at].time_ms - bars[at - 1].time_ms) as f64;

    (elapsed / usual_ms).clamp(SHORTEST_GAP, LONGEST_GAP) as f32
}

fn scaled(scaler: &Scaler, row: &Row) -> Vec<f32> {
    row.iter()
        .enumerate()
        .map(|(at, value)| scaler.scaled(at, *value))
        .collect()
}

/// What the dendritic read-out takes for a bar.
fn dendritic_inputs(regime: &Regime) -> Vec<f32> {
    let mut inputs = vec![0.0f32; DENDRITIC_INPUTS];

    for (at, value) in regime.outlook().row().iter().enumerate() {
        inputs[at] = *value as f32;
    }

    if let Some(codes) = regime.lived().last() {
        one_hot(codes, &mut inputs[OUTLOOK..]);
    }

    inputs
}

fn one_hot(codes: &Codes, into: &mut [f32]) {
    let widths = [5usize, 3, 3, 3, 3];
    let mut begin = 0;

    for (code, width) in codes.iter().zip(widths) {
        if (*code as usize) < width {
            into[begin + *code as usize] = 1.0;
        }

        begin += width;
    }
}

/// Mean squared error of `outs` against `targets` on the samples `rows`.
fn mean_square(outs: &[f32], targets: &[f32], rows: &[usize]) -> f64 {
    let mut sum = 0.0;
    let mut count = 0.0;

    for at in rows {
        for k in 0..OUTPUTS {
            let error = f64::from(outs[at * OUTPUTS + k] - targets[at * OUTPUTS + k]);

            sum += error * error;
            count += 1.0;
        }
    }

    if count > 0.0 { sum / count } else { f64::MAX }
}

/// Fits a ridge read-out, choosing the penalty on the checked rows, and
/// returns it with its outputs for every row.
fn read_out(
    x: &[f32],
    y: &[f32],
    inputs: usize,
    count: usize,
    fitted: &[usize],
    checked: &[usize],
) -> (Ridge, Vec<f32>) {
    let is_fitted = {
        let mut marks = vec![false; count];

        for at in fitted {
            marks[*at] = true;
        }

        marks
    };

    let outputs_of = |ridge: &Ridge| -> Vec<f32> {
        (0..count)
            .flat_map(|at| ridge.apply(&x[at * inputs..(at + 1) * inputs]))
            .collect()
    };

    let mut best: Option<(f64, f64)> = None;

    for penalty in PENALTIES {
        let ridge = Ridge::fit(x, y, inputs, OUTPUTS, penalty, |at| is_fitted[at]);
        let error = mean_square(&outputs_of(&ridge), y, checked);

        if best.is_none_or(|(_, lowest)| error < lowest) {
            best = Some((penalty, error));
        }
    }

    let penalty = best.map_or(10.0, |(penalty, _)| penalty);

    let mut both = vec![false; count];

    for at in fitted.iter().chain(checked) {
        both[*at] = true;
    }

    let ridge = Ridge::fit(x, y, inputs, OUTPUTS, penalty, |at| both[at]);
    let outs = outputs_of(&ridge);

    (ridge, outs)
}

impl Forecaster {
    pub fn is_sound(&self) -> bool {
        self.cell.is_sound()
            && self.cell_state.len() == self.cell.shape().units
            && self.reservoir.is_sound()
            && self.reservoir.fits(&self.activity)
            && self.spiking.is_sound()
            && self.spiking.inputs == self.reservoir.neurons
            && self.spiking.outputs == OUTPUTS
            && self.dendritic.is_sound()
            && self.dendritic.outputs == OUTPUTS
            && self.scale.mean.len() == OUTPUTS
            && self.scale.spread.len() == OUTPUTS
            && self.scale.spread.iter().all(|s| s.is_finite() && *s > 0.)
            && self.bands.len() == 4
            && self.bands.iter().all(|b| b.len() == STEPS)
            && self.latest.iter().all(|v| v.len() == OUTPUTS)
            && self.gap_ms.is_finite()
            && self.gap_ms > 0.
    }
    /// Trains the three networks on the first part of `bars` and measures
    /// them on the rest. `progress` is told a fraction from 0 to 1.
    pub fn train(
        bars: &[Bar],
        settings: &PathSettings,
        progress: &dyn Fn(f32),
    ) -> Result<(Forecaster, PathReport), Error> {
        let began = Instant::now();

        for (index, bar) in bars.iter().enumerate() {
            if let Some(problem) = bar.problem() {
                return Err(Error::BadBar { index, problem });
            }
        }

        let (bars, _) = bars::tidy(bars.to_vec());
        let count = bars.len();

        let held_out =
            (count as f64 * f64::from(settings.test_share.clamp(0.05, 0.5))).round() as usize;
        let first_test = count.saturating_sub(held_out);

        if count < settings.fewest_bars()
            || first_test < WARMUP + FEWEST_FITTED + STEPS
            || held_out < FEWEST_SCORED + STEPS
        {
            return Err(Error::TooFewBars {
                needed: settings.fewest_bars(),
                found: count,
            });
        }

        progress(0.0);

        let rows = features::table(&bars);
        let scaler = Scaler::fit(
            rows[..first_test].iter().map(|row| row.as_slice()),
            FEATURES,
        );
        let shown: Vec<f32> = rows.iter().flat_map(|row| scaled(&scaler, row)).collect();

        // Samples whose targets are known before the held-out bars begin.
        let known = first_test - STEPS;
        let trainable: Vec<usize> = (WARMUP..known).collect();
        let first_checked = known - ((known - WARMUP) as f64 * CHECKED) as usize;
        let fitted: Vec<usize> = (WARMUP..first_checked).collect();
        let checked: Vec<usize> = (first_checked..known).collect();
        let tested: Vec<usize> = (first_test..count - STEPS).collect();

        let raw: Vec<Option<[f64; OUTPUTS]>> = (0..count).map(|at| targets(&bars, at)).collect();

        let scale = {
            let mut mean = vec![0.0; OUTPUTS];
            let mut squares = vec![0.0; OUTPUTS];

            for at in &trainable {
                if let Some(target) = &raw[*at] {
                    for k in 0..OUTPUTS {
                        mean[k] += target[k];
                        squares[k] += target[k] * target[k];
                    }
                }
            }

            let seen = trainable.len().max(1) as f64;

            for k in 0..OUTPUTS {
                mean[k] /= seen;
                squares[k] = (squares[k] / seen - mean[k] * mean[k]).max(0.0).sqrt();

                // A target that never varied, such as the opening gap of
                // bars that always open at the close before, is forecast
                // as its mean whatever the network outputs.
                squares[k] = squares[k].max(1e-12);
            }

            TargetScale {
                mean,
                spread: squares,
            }
        };

        let mut standard = vec![0.0f32; count * OUTPUTS];

        for (at, target) in raw.iter().enumerate() {
            if let Some(target) = target {
                for k in 0..OUTPUTS {
                    standard[at * OUTPUTS + k] = scale.standard(k, target[k]);
                }
            }
        }

        let gaps_ms: Vec<f64> = bars[..first_test]
            .windows(2)
            .map(|two| (two[1].time_ms - two[0].time_ms) as f64)
            .collect();
        let gap_ms = stats::quantile(&gaps_ms, 0.5).max(1.0);
        let gaps: Vec<f32> = (0..count).map(|at| gap(&bars, at, gap_ms)).collect();

        // The dendritic and the spiking network live the bars in order.
        let dendritic_began = Instant::now();

        let mut regime = Regime::new(Buckets::fit(&rows[..first_test]));
        let mut dendritic_x = Vec::with_capacity(count * DENDRITIC_INPUTS);

        for (at, row) in rows.iter().enumerate() {
            regime.observe(row);
            dendritic_x.extend(dendritic_inputs(&regime));

            if at % 64 == 0 {
                progress(0.05 + 0.15 * at as f32 / count as f32);
            }
        }

        let (dendritic, dendritic_outs) = read_out(
            &dendritic_x,
            &standard,
            DENDRITIC_INPUTS,
            count,
            &fitted,
            &checked,
        );

        let dendritic_ms = dendritic_began.elapsed().as_secs_f64() * 1e3;

        let spiking_began = Instant::now();

        let reservoir = Reservoir::new(settings.neurons, FEATURES, settings.seed);
        let mut activity = reservoir.fresh();
        let mut spiking_x = Vec::with_capacity(count * settings.neurons);
        let mut spikes = 0u64;

        for at in 0..count {
            reservoir.play(&mut activity, &shown[at * FEATURES..(at + 1) * FEATURES]);
            spiking_x.extend_from_slice(&activity.trace);
            spikes += u64::from(activity.spikes);

            if at % 64 == 0 {
                progress(0.2 + 0.15 * at as f32 / count as f32);
            }
        }

        let (spiking, spiking_outs) = read_out(
            &spiking_x,
            &standard,
            settings.neurons,
            count,
            &fitted,
            &checked,
        );

        let spiking_ms = spiking_began.elapsed().as_secs_f64() * 1e3;

        // The liquid network.
        let liquid_began = Instant::now();

        let mut fit = vec![0.0f32; count * OUTPUTS];
        let mut check = vec![0.0f32; count * OUTPUTS];

        for at in &fitted {
            fit[at * OUTPUTS..(at + 1) * OUTPUTS].fill(1.0);
        }

        for at in &checked {
            check[at * OUTPUTS..(at + 1) * OUTPUTS].fill(1.0);
        }

        let shape = Shape {
            inputs: FEATURES,
            units: settings.units,
            backbone: settings.units,
            heads: vec![Head::Number; OUTPUTS],
        };

        let mut cell = Cfc::new(shape, settings.seed);

        let lesson = Lesson {
            inputs: &shown,
            gaps: &gaps,
            targets: &standard,
            fit: &fit,
            check: &check,
        };

        let training = Training {
            epochs: settings.epochs,
            window: settings.window,
            seed: settings.seed,
            ..Training::default()
        };

        let losses = cell.train(&lesson, &training, &mut |epoch, _| {
            progress(0.35 + 0.6 * (epoch + 1) as f32 / settings.epochs.max(1) as f32);
        });

        let mut cell_state = cell.fresh_state();
        let liquid_outs = cell.run(&mut cell_state, &shown, &gaps);

        let liquid_ms = liquid_began.elapsed().as_secs_f64() * 1e3;

        // The ensemble weights each by its error on the checked samples.
        let errors = [
            mean_square(&liquid_outs, &standard, &checked),
            mean_square(&dendritic_outs, &standard, &checked),
            mean_square(&spiking_outs, &standard, &checked),
        ];

        let inverse: Vec<f64> = errors.iter().map(|error| 1.0 / error.max(1e-6)).collect();
        let total: f64 = inverse.iter().sum();
        let weights = [
            (inverse[0] / total) as f32,
            (inverse[1] / total) as f32,
            (inverse[2] / total) as f32,
        ];

        let ensemble_outs: Vec<f32> = (0..count * OUTPUTS)
            .map(|k| {
                weights[0] * liquid_outs[k]
                    + weights[1] * dendritic_outs[k]
                    + weights[2] * spiking_outs[k]
            })
            .collect();

        let band = |outs: &[f32]| -> Vec<(f64, f64)> {
            (1..=STEPS)
                .map(|step| {
                    let k = CLOSE * STEPS + step - 1;

                    let errors: Vec<f64> = checked
                        .iter()
                        .filter_map(|at| {
                            let actual = raw[*at]?[k];

                            Some(actual - scale.value(k, outs[at * OUTPUTS + k]))
                        })
                        .collect();

                    (stats::quantile(&errors, 0.1), stats::quantile(&errors, 0.9))
                })
                .collect()
        };

        let bands = vec![
            band(&liquid_outs),
            band(&dendritic_outs),
            band(&spiking_outs),
            band(&ensemble_outs),
        ];

        // Held-out metrics.
        let measure = |net: Net, outs: &[f32]| -> NetMetrics {
            let (band_low, band_high) = bands[net.slot()][0];

            let value = |at: usize, part: usize, step: usize| {
                let k = part * STEPS + step - 1;

                scale.value(k, outs[at * OUTPUTS + k])
            };

            let steps = SCORED_STEPS
                .iter()
                .map(|step| {
                    step_score(
                        *step,
                        tested.iter().map(|at| {
                            let actual = raw[*at].map_or(0.0, |t| t[CLOSE * STEPS + step - 1]);

                            (value(*at, CLOSE, *step), actual)
                        }),
                    )
                })
                .collect();

            let mut band_hits = 0usize;
            let mut volume_error = 0.0;
            let mut volumes = 0usize;

            for at in &tested {
                let from = bars[*at].close;
                let next = &bars[*at + 1];
                let predicted = value(*at, CLOSE, 1);

                let low = from * (predicted + band_low).exp();
                let high = from * (predicted + band_high).exp();

                if next.close >= low && next.close <= high {
                    band_hits += 1;
                }

                let usual = usual_volume(&bars, *at);

                if usual > 0.0 && next.volume > 0.0 {
                    let predicted = usual * value(*at, VOLUME, 1).exp();

                    volume_error += (predicted - next.volume).abs() / next.volume;
                    volumes += 1;
                }
            }

            NetMetrics {
                net,
                steps,
                band_hit: (!tested.is_empty()).then(|| band_hits as f64 / tested.len() as f64),
                volume_error: (volumes > 0).then(|| volume_error / volumes as f64),
                parameters: 0,
                neurons: 0,
                weight: 0.0,
                train_ms: 0.0,
                losses: Vec::new(),
                facts: Vec::new(),
            }
        };

        let growth = regime.growth();

        let mut liquid = measure(Net::Liquid, &liquid_outs);

        liquid.parameters = cell.shape().weights();
        liquid.neurons = settings.units;
        liquid.weight = weights[0];
        liquid.train_ms = liquid_ms;
        liquid.losses = losses;
        liquid.facts = vec![
            ("Units".to_string(), settings.units.to_string()),
            ("Epochs".to_string(), settings.epochs.to_string()),
            ("Usual gap".to_string(), gap_text(gap_ms)),
        ];

        let mut dendritic_metrics = measure(Net::Dendritic, &dendritic_outs);

        dendritic_metrics.parameters = (DENDRITIC_INPUTS + 1) * OUTPUTS;
        dendritic_metrics.neurons = growth.neurons;
        dendritic_metrics.weight = weights[1];
        dendritic_metrics.train_ms = dendritic_ms;
        dendritic_metrics.facts = vec![
            ("Branches".to_string(), growth.segments.to_string()),
            ("Synapses".to_string(), growth.synapses.to_string()),
            (
                "Expected right".to_string(),
                format!(
                    "{:.0}% (commonest {:.0}%)",
                    growth.expected_right * 100.0,
                    growth.baseline_right * 100.0
                ),
            ),
        ];

        let mut spiking_metrics = measure(Net::Spiking, &spiking_outs);

        spiking_metrics.parameters = (settings.neurons + 1) * OUTPUTS;
        spiking_metrics.neurons = settings.neurons;
        spiking_metrics.weight = weights[2];
        spiking_metrics.train_ms = spiking_ms;
        spiking_metrics.facts = vec![(
            "Spikes per bar".to_string(),
            format!("{:.0}", spikes as f64 / count as f64),
        )];

        let mut ensemble = measure(Net::Ensemble, &ensemble_outs);

        ensemble.weight = 1.0;
        ensemble.facts = vec![(
            "Weights".to_string(),
            format!(
                "LNN {:.0}% · DANN {:.0}% · SNN {:.0}%",
                weights[0] * 100.0,
                weights[1] * 100.0,
                weights[2] * 100.0
            ),
        )];

        let baseline = SCORED_STEPS
            .iter()
            .map(|step| {
                step_score(
                    *step,
                    tested.iter().map(|at| {
                        let actual = raw[*at].map_or(0.0, |t| t[CLOSE * STEPS + step - 1]);
                        let before = at.saturating_sub(*step);

                        // The same move again: a direction, and a size.
                        (log_ratio(bars[*at].close, bars[before].close), actual)
                    }),
                )
            })
            .collect();

        let last = count - 1;
        let latest = |outs: &[f32]| outs[last * OUTPUTS..(last + 1) * OUTPUTS].to_vec();

        let forecaster = Forecaster {
            settings: settings.clone(),
            scaler,
            scale,
            gap_ms,
            cell,
            cell_state,
            regime,
            dendritic,
            reservoir,
            activity,
            spiking,
            weights,
            bands,
            recent: bars[count.saturating_sub(KEPT)..].to_vec(),
            latest: [
                latest(&liquid_outs),
                latest(&dendritic_outs),
                latest(&spiking_outs),
            ],
        };

        let report = PathReport {
            bars: count,
            train_bars: first_test,
            scored: tested.len(),
            metrics: vec![liquid, dendritic_metrics, spiking_metrics, ensemble],
            baseline,
            growth,
            total_ms: began.elapsed().as_secs_f64() * 1e3,
        };

        progress(1.0);

        Ok((forecaster, report))
    }

    /// Takes a bar that closed after the latest one seen: every network
    /// moves on and the forecasts are rewritten. The dendritic network
    /// learns from it; the trained weights do not change. Returns false
    /// for a bar that is not later than the latest or cannot be used.
    pub fn advance(&mut self, bar: Bar) -> bool {
        let latest = self.recent.last().map_or(i64::MIN, |bar| bar.time_ms);

        if bar.time_ms <= latest || bar.problem().is_some() {
            return false;
        }

        self.recent.push(bar);

        if self.recent.len() > KEPT {
            self.recent.remove(0);
        }

        let at = self.recent.len() - 1;
        let row = features::row(&self.recent, at);
        let shown = scaled(&self.scaler, &row);

        self.regime.observe(&row);
        self.latest[1] = self.dendritic.apply(&dendritic_inputs(&self.regime));

        let gap = gap(&self.recent, at, self.gap_ms);

        self.latest[0] = self.cell.run(&mut self.cell_state, &shown, &[gap]);

        self.reservoir.play(&mut self.activity, &shown);
        self.latest[2] = self.spiking.apply(&self.activity.trace);

        true
    }

    /// A copy that has lived `bars` from the start instead: the same
    /// trained weights, run on other bars, such as ones read off a
    /// picture of a chart. Prices may be on any scale.
    pub fn replayed(&self, bars: &[Bar]) -> Forecaster {
        let mut copy = self.clone();

        copy.cell_state = copy.cell.fresh_state();
        copy.activity = copy.reservoir.fresh();
        copy.regime = Regime::new(self.regime.buckets().clone());
        copy.recent.clear();

        for bar in bars {
            copy.advance(*bar);
        }

        copy
    }

    /// The next [`STEPS`] bars as `net` forecasts them.
    pub fn path(&self, net: Net) -> Path {
        let outs: Vec<f32> = match net {
            Net::Ensemble => (0..OUTPUTS)
                .map(|k| {
                    self.weights[0] * self.latest[0][k]
                        + self.weights[1] * self.latest[1][k]
                        + self.weights[2] * self.latest[2][k]
                })
                .collect(),
            net => self.latest[net.slot()].clone(),
        };

        let last = self.recent.last().copied();
        let from = last.map_or(0.0, |bar| bar.close);
        let usual = if self.recent.is_empty() {
            0.0
        } else {
            usual_volume(&self.recent, self.recent.len() - 1)
        };

        let value = |part: usize, step: usize| {
            let k = part * STEPS + step - 1;

            self.scale.value(k, outs[k])
        };

        let mut candles = Vec::with_capacity(STEPS);
        let mut before = from;

        for step in 1..=STEPS {
            let open = before * value(OPENING, step).exp();
            let ratio = value(CLOSE, step);
            let close = from * ratio.exp();
            let (band_low, band_high) = self.bands[net.slot()][step - 1];
            let high = open.max(close) * value(UPPER, step).max(0.0).exp();
            let low = open.min(close) * (-value(LOWER, step).max(0.0)).exp();
            let volume = if usual > 0.0 {
                usual * value(VOLUME, step).exp()
            } else {
                0.0
            };

            candles.push(PredictedCandle {
                step,
                open,
                high,
                low,
                close,
                volume,
                close_low: from * (ratio + band_low).exp(),
                close_high: from * (ratio + band_high).exp(),
            });

            before = close;
        }

        Path {
            net,
            after_time_ms: last.map_or(0, |bar| bar.time_ms),
            from_close: from,
            candles,
        }
    }

    /// The dendritic network's own account of what comes next.
    pub fn account(&self) -> Account {
        self.regime.account()
    }

    pub fn growth(&self) -> Growth {
        self.regime.growth()
    }

    /// Spikes in the reservoir over the latest bar.
    pub fn spikes(&self) -> u32 {
        self.activity.spikes
    }

    pub fn weights(&self) -> [f32; 3] {
        self.weights
    }

    pub fn settings(&self) -> &PathSettings {
        &self.settings
    }

    /// The time of the latest bar seen.
    pub fn latest_time_ms(&self) -> Option<i64> {
        self.recent.last().map(|bar| bar.time_ms)
    }
}

fn gap_text(ms: f64) -> String {
    let minutes = ms / 60_000.0;

    if minutes < 60.0 {
        format!("{minutes:.0} min")
    } else if minutes < 60.0 * 24.0 {
        format!("{:.1} h", minutes / 60.0)
    } else {
        format!("{:.1} days", minutes / 1_440.0)
    }
}

/// Scores predicted against actual log returns at one step.
fn step_score(step: usize, pairs: impl Iterator<Item = (f64, f64)>) -> StepScore {
    let mut scored = 0usize;
    let mut right = 0usize;
    let mut error = 0.0;
    let mut squares = 0.0;
    let mut zero_squares = 0.0;
    let mut all = 0usize;

    for (predicted, actual) in pairs {
        all += 1;
        error += ((predicted.exp() - actual.exp()) * 100.0).abs();
        squares += (predicted - actual).powi(2);
        zero_squares += actual.powi(2);

        if predicted != 0.0 && actual != 0.0 {
            scored += 1;

            if (predicted > 0.0) == (actual > 0.0) {
                right += 1;
            }
        }
    }

    // Overlapping returns over `step` bars count for one in `step`.
    let (effective_right, effective) = if step > 1 {
        let of = (scored / step).max(1) as u64;

        ((right as u64 * of) / scored.max(1) as u64, of)
    } else {
        (right as u64, scored as u64)
    };

    let accuracy = (scored > 0).then(|| right as f64 / scored as f64);
    let interval = (scored > 0).then(|| stats::wilson(effective_right, effective, Z_99));

    StepScore {
        step,
        scored,
        right,
        accuracy,
        low: interval.map(|(low, _)| low),
        high: interval.map(|(_, high)| high),
        p_value: (scored > 0).then(|| stats::binomial_two_sided(effective_right, effective)),
        error_pct: if all > 0 { error / all as f64 } else { 0.0 },
        skill: if zero_squares > 0.0 {
            1.0 - squares / zero_squares
        } else {
            0.0
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synthetic;

    fn quick() -> PathSettings {
        PathSettings {
            epochs: 3,
            units: 8,
            neurons: 48,
            ..PathSettings::default()
        }
    }

    #[test]
    fn every_network_is_trained_measured_and_forecasts_ten_candles() {
        let bars = synthetic::patterned(600, 3);
        let (forecaster, report) = Forecaster::train(&bars, &quick(), &|_| {}).unwrap();

        assert_eq!(report.metrics.len(), 4);
        assert!(report.scored > 0);

        for metrics in &report.metrics {
            assert_eq!(metrics.steps.len(), SCORED_STEPS.len());
            assert!(metrics.steps.iter().all(|score| score.accuracy.is_some()));
        }

        let weights: f32 = forecaster.weights().iter().sum();

        assert!((weights - 1.0).abs() < 1e-4);

        for net in Net::ALL {
            let path = forecaster.path(net);

            assert_eq!(path.candles.len(), STEPS);

            for candle in &path.candles {
                assert!(
                    candle.high >= candle.open.max(candle.close) - 1e-9,
                    "{candle:?}"
                );
                assert!(
                    candle.low <= candle.open.min(candle.close) + 1e-9,
                    "{candle:?}"
                );
                assert!(candle.close.is_finite() && candle.close > 0.0);
            }
        }
    }

    #[test]
    fn a_new_bar_moves_the_forecast_on() {
        let bars = synthetic::patterned(700, 4);
        let (mut forecaster, _) = Forecaster::train(&bars[..650], &quick(), &|_| {}).unwrap();

        let before = forecaster.path(Net::Liquid);

        assert!(forecaster.advance(bars[650]));
        assert!(!forecaster.advance(bars[650]), "the same bar twice");

        let after = forecaster.path(Net::Liquid);

        assert_eq!(after.after_time_ms, bars[650].time_ms);
        assert_ne!(before, after);
    }

    #[test]
    fn too_few_bars_are_refused() {
        let bars = synthetic::random_walk(100, 1);

        assert!(matches!(
            Forecaster::train(&bars, &quick(), &|_| {}),
            Err(Error::TooFewBars { .. })
        ));
    }

    #[test]
    fn a_trained_model_runs_on_other_bars_at_another_price() {
        let bars = synthetic::patterned(600, 3);
        let (forecaster, _) = Forecaster::train(&bars, &quick(), &|_| {}).unwrap();

        let other: Vec<Bar> = synthetic::random_walk(60, 9)
            .into_iter()
            .map(|bar| Bar {
                open: bar.open * 50.0,
                high: bar.high * 50.0,
                low: bar.low * 50.0,
                close: bar.close * 50.0,
                ..bar
            })
            .collect();

        let replayed = forecaster.replayed(&other);
        let path = replayed.path(Net::Ensemble);

        let last = other.last().unwrap().close;

        assert_eq!(path.from_close, last);
        assert!(
            path.candles
                .iter()
                .all(|candle| (candle.close / last - 1.0).abs() < 0.5)
        );
    }

    #[test]
    fn the_targets_say_what_the_next_bars_did() {
        let bars: Vec<Bar> = (0..15)
            .map(|at| Bar {
                time_ms: at,
                open: 100.0 + at as f64,
                high: 102.0 + at as f64,
                low: 99.0 + at as f64,
                close: 101.0 + at as f64,
                volume: 10.0,
            })
            .collect();

        let target = targets(&bars, 2).unwrap();

        assert!((target[CLOSE * STEPS] - (104.0f64 / 103.0).ln()).abs() < 1e-12);
        assert!((target[UPPER * STEPS] - (105.0f64 / 104.0).ln()).abs() < 1e-12);
        assert!((target[LOWER * STEPS] - (103.0f64 / 102.0).ln()).abs() < 1e-12);
        assert!(target[VOLUME * STEPS].abs() < 1e-12);
        assert!(targets(&bars, 5).is_none());
    }
}
