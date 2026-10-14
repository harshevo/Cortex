//! The analysis: train both networks on the first part of a series,
//! score them on the last part, which neither the training nor any
//! choice of scale or bucket has touched, and say plainly what was found.
//!
//! The order of work, for `n` bars and a horizon of `h` bars:
//!
//! 1. The last `test_share` of the bars is set aside. Everything fitted
//!    (the scale of each number, the bucket edges, the usual gap between
//!    bars, the weights) comes from the bars before it, and only from
//!    targets that are known before the held-out part begins.
//! 2. The dendritic network lives every bar in order, and what it
//!    expects of each next bar is recorded as it goes.
//! 3. Two liquid networks are trained: one on the bar numbers alone, one
//!    with the dendritic network's expectations added. The last 15% of
//!    the training targets are not fitted but used to pick which epoch's
//!    weights to keep.
//! 4. On the held-out part each predictor is scored on the return over
//!    the next `h` bars: how often it called the direction, and how far
//!    off its predicted return was. Three baselines are scored beside
//!    them: always up, same as the last `h` bars, and no change.
//! 5. A verdict. Prices are close to a random walk, so the expected
//!    verdict is that there is no edge, and the bar for saying otherwise
//!    is high: the combined model must beat both a coin and the best
//!    baseline by more than a 99% interval allows for chance.
//!
//! What this is not. It is not advice, and nothing here says to buy or
//! sell. An accuracy measured on one held-out stretch says how the model
//! did on that stretch; markets change, and costs of trading are not
//! counted at all. Run on many files or many settings, some will show an
//! edge by chance alone: at the 99% level, about one in two hundred.

use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::bars::{self, Bar};
use crate::cfc::{Cfc, EpochLoss, Head, Lesson, Shape, Training};
use crate::features::{self, FEATURES, KEPT, Row, Scaler};
use crate::regime::{self, Account, Buckets, Growth, OUTLOOK, Outlook, Regime};
use crate::stats;

/// Bars at the start whose numbers rest on too little history to be
/// worth fitting to.
const WARMUP: usize = 20;

/// Fewest training targets, and fewest held-out bars scored.
const FEWEST_FITTED: usize = 60;
const FEWEST_SCORED: usize = 20;

/// Share of the training targets held back to pick the epoch.
const CHECKED: f64 = 0.15;

/// The shortest and longest a gap between bars counts as, in usual gaps.
const SHORTEST_GAP: f64 = 0.05;
const LONGEST_GAP: f64 = 10.0;

/// A standardised target is cut off this far out.
const FURTHEST_TARGET: f64 = 5.0;

/// Fewest independent scored bars for a verdict other than none.
const FEWEST_FOR_VERDICT: u64 = 30;

/// Standard deviations for the 99% and 99.9% intervals.
const Z_99: f64 = 2.576;
const Z_999: f64 = 3.291;

/// How to analyse.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Bars ahead that are predicted.
    pub horizon: usize,

    /// Units of state in the liquid network, and in its backbone.
    pub units: usize,

    /// Passes over the training part.
    pub epochs: usize,

    /// Bars the gradient is followed back through.
    pub window: usize,

    /// Share of the bars, the latest ones, held out for scoring.
    pub test_share: f32,

    /// The same seed and bars give the same result.
    pub seed: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            horizon: 1,
            units: 24,
            epochs: 30,
            window: 32,
            test_share: 0.25,
            seed: 1,
        }
    }
}

impl Settings {
    fn check(&self) -> Result<(), Error> {
        let bad = |what: &str| Err(Error::BadSettings(what.to_string()));

        if self.horizon == 0 || self.horizon > 500 {
            return bad("the horizon must be from 1 to 500 bars");
        }

        if !(2..=256).contains(&self.units) {
            return bad("units must be from 2 to 256");
        }

        if !(1..=1_000).contains(&self.epochs) {
            return bad("epochs must be from 1 to 1000");
        }

        if !(2..=1_024).contains(&self.window) {
            return bad("the window must be from 2 to 1024 bars");
        }

        if !(0.05..=0.5).contains(&self.test_share) {
            return bad("the held-out share must be from 0.05 to 0.5");
        }

        Ok(())
    }

    /// The fewest bars these settings can be run on.
    pub fn fewest_bars(&self) -> usize {
        let share = self.test_share.clamp(0.05, 0.5) as f64;

        let for_training = (FEWEST_FITTED + self.horizon) as f64 / (1.0 - share);
        let for_scoring = (FEWEST_SCORED + self.horizon) as f64 / share;

        for_training.max(for_scoring).ceil() as usize + 1
    }
}

/// What the analysis is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// Working out the numbers for each bar.
    Features,

    /// The dendritic network living the bars.
    Dendritic,

    /// Training the liquid network on its own.
    Liquid,

    /// Training the liquid network with the dendritic network's
    /// expectations added.
    Combined,

    Scoring,
    Done,
}

/// How far along the analysis is.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Progress {
    pub stage: Stage,

    /// Of the whole analysis, from 0 to 1. Never goes down.
    pub fraction: f32,
}

/// The share of the whole each stage begins at, by how long each takes.
fn begins(stage: Stage) -> (f32, f32) {
    match stage {
        Stage::Features => (0.0, 0.02),
        Stage::Dendritic => (0.02, 0.3),
        Stage::Liquid => (0.3, 0.63),
        Stage::Combined => (0.63, 0.97),
        Stage::Scoring => (0.97, 1.0),
        Stage::Done => (1.0, 1.0),
    }
}

/// What made a prediction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Predictor {
    /// The liquid network on the bar numbers alone.
    Liquid,

    /// The dendritic network's surest expected move, on the bars where
    /// it expects one.
    Dendritic,

    /// The liquid network given the dendritic network's expectations.
    Combined,

    /// Baseline: up every time, by the training part's mean return.
    AlwaysUp,

    /// Baseline: the same return as over the bars just gone.
    Persistence,

    /// Baseline: no change. It calls no direction.
    Zero,
}

impl Predictor {
    pub fn name(self) -> &'static str {
        match self {
            Predictor::Liquid => "liquid network alone",
            Predictor::Dendritic => "dendritic network alone",
            Predictor::Combined => "liquid + dendritic",
            Predictor::AlwaysUp => "baseline: always up",
            Predictor::Persistence => "baseline: same as last",
            Predictor::Zero => "baseline: zero return",
        }
    }
}

/// How one predictor did on the held-out bars.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Score {
    pub predictor: Predictor,
    pub name: String,

    /// Held-out bars whose return over the horizon is known.
    pub bars: usize,

    /// Of those, the ones where it called a direction and the price
    /// moved at all. Accuracy is over these.
    pub scored: usize,
    pub right: usize,

    /// `right / scored`. Absent when nothing was scored.
    pub accuracy: Option<f64>,

    /// The 99% interval for the accuracy.
    pub accuracy_low: Option<f64>,
    pub accuracy_high: Option<f64>,

    /// The chance of an accuracy this far from 50%, either side, from
    /// calling at random. Two-sided exact binomial.
    pub p_value: Option<f64>,

    /// The mean distance between the predicted and the actual log
    /// return, over all of `bars`.
    pub mean_abs_error: f64,
}

/// Whether the combined model showed anything on the held-out bars.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Edge {
    /// Nothing that chance or a simple baseline does not explain. The
    /// usual, and expected, result.
    None,

    /// It beat a coin and the best baseline by more than the 99%
    /// interval. Chance alone gives this about once in two hundred runs.
    Weak,

    /// It beat both by more than the 99.9% interval.
    Present,
}

/// The finding, in a word and in a sentence or two.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Verdict {
    pub edge: Edge,

    /// What was found, in plain English, ready to show.
    pub text: String,

    /// The baseline with the best directional accuracy.
    pub best_baseline: Predictor,

    /// The accuracy the combined model had to beat: the best baseline's
    /// or 50%, whichever is higher.
    pub accuracy_to_beat: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Up,
    Down,

    /// The model gives exactly even odds.
    Flat,
}

/// What the combined model makes of the bars after the latest one. A
/// forecast, with the measured record beside it in [`Analysis`]; not a
/// recommendation to do anything.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Forecast {
    /// The time of the latest bar seen.
    pub after_time_ms: i64,

    /// Bars ahead that the forecast is for.
    pub horizon: usize,

    /// The log return it expects over the horizon.
    pub expected_return: f64,

    /// From `probability_up`. The two come from separate outputs of the
    /// network, so `direction` and the sign of `expected_return` can
    /// disagree.
    pub direction: Direction,

    /// The odds it gives its own direction, from 0.5 to 1. This is the
    /// model's own figure, not a measured hit rate.
    pub confidence: f32,

    pub probability_up: f32,

    /// The dendritic network's own account, for showing why.
    pub dendritic: Account,
}

/// Predictions on the held-out bars, for a chart.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Series {
    /// The index of the first held-out bar. Entry `i` of each list
    /// below was made at bar `first_bar + i`, about the return from
    /// that bar's close to the close `horizon` bars later.
    pub first_bar: usize,

    /// The combined model's.
    pub predicted_return: Vec<f32>,
    pub probability_up: Vec<f32>,

    /// What happened. Absent for the last `horizon` bars.
    pub actual_return: Vec<Option<f32>>,
}

/// How long each part took, in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Timings {
    pub features_ms: f64,
    pub dendritic_ms: f64,

    /// Both liquid networks.
    pub training_ms: f64,
    pub total_ms: f64,
}

/// Everything measured.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Analysis {
    /// Bars analysed, after any with the same time were dropped.
    pub bars: usize,

    /// Bars before the held-out part.
    pub train_bars: usize,
    pub horizon: usize,

    /// One per [`Predictor`], on the held-out bars.
    pub scores: Vec<Score>,
    pub verdict: Verdict,
    pub forecast: Forecast,
    pub series: Series,

    /// Loss after each epoch, for each of the two liquid networks.
    pub liquid_losses: Vec<EpochLoss>,
    pub combined_losses: Vec<EpochLoss>,

    /// The regime of each bar, as an index into `regime_names`.
    pub regimes: Vec<u8>,
    pub regime_names: Vec<String>,

    /// How the dendritic network grew over the whole series.
    pub growth: Growth,

    /// Things a reader should know about this run, in plain sentences.
    pub notes: Vec<String>,

    pub timings: Timings,
}

impl Analysis {
    pub fn score(&self, predictor: Predictor) -> Option<&Score> {
        self.scores
            .iter()
            .find(|score| score.predictor == predictor)
    }
}

/// What a kept model did with new bars.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Update {
    /// Bars taken in.
    pub added: usize,

    /// Bars left out: not later than the latest seen, or not usable.
    pub ignored: usize,
}

/// A trained pair of networks and everything needed to show them the
/// next bar: scales, bucket edges, the liquid network's state, and the
/// last few bars. Keep it with [`Model::to_json`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Model {
    settings: Settings,
    scaler: Scaler,
    outlook_scaler: Scaler,
    target_mean: f64,
    target_spread: f64,

    /// The usual time between bars in training.
    gap_ms: f64,
    cell: Cfc,
    state: Vec<f32>,
    regime: Regime,

    /// The latest bars, enough to work out the next one's numbers.
    recent: Vec<Bar>,
    forecast: Forecast,
}

/// The time since the bar before, in usual gaps.
fn gap(bars: &[Bar], at: usize, usual_ms: f64) -> f32 {
    if at == 0 {
        return 1.0;
    }

    let elapsed = (bars[at].time_ms - bars[at - 1].time_ms) as f64;

    (elapsed / usual_ms).clamp(SHORTEST_GAP, LONGEST_GAP) as f32
}

/// One bar's numbers as the combined network takes them.
fn inputs(scaler: &Scaler, outlook_scaler: &Scaler, row: &Row, outlook: &Outlook) -> Vec<f32> {
    let bar = row.iter().enumerate().map(|(at, x)| scaler.scaled(at, *x));

    let outlook = outlook.row();
    let outlook = outlook
        .iter()
        .enumerate()
        .map(|(at, x)| outlook_scaler.scaled(at, *x));

    bar.chain(outlook).collect()
}

fn odds(out: f32) -> f32 {
    1.0 / (1.0 + (-out).exp())
}

impl Model {
    /// Trains on `bars` and measures the result on their last part.
    ///
    /// Bars need not be sorted; any with the same time as another are
    /// dropped. `progress` is called from this thread, often, with a
    /// fraction that never goes down.
    pub fn train(
        bars: &[Bar],
        settings: &Settings,
        progress: &dyn Fn(Progress),
    ) -> Result<(Model, Analysis), Error> {
        let began = Instant::now();

        settings.check()?;

        for (index, bar) in bars.iter().enumerate() {
            if let Some(problem) = bar.problem() {
                return Err(Error::BadBar { index, problem });
            }
        }

        let (bars, dropped) = bars::tidy(bars.to_vec());

        let tell = |stage: Stage, within: f32| {
            let (begin, end) = begins(stage);

            progress(Progress {
                stage,
                fraction: begin + (end - begin) * within.clamp(0.0, 1.0),
            });
        };

        let count = bars.len();
        let horizon = settings.horizon;

        let held_out = (count as f64 * settings.test_share as f64).round() as usize;
        let first_test = count.saturating_sub(held_out);

        if count < settings.fewest_bars()
            || first_test < FEWEST_FITTED + horizon
            || held_out < FEWEST_SCORED + horizon
        {
            return Err(Error::TooFewBars {
                needed: settings.fewest_bars(),
                found: count,
            });
        }

        tell(Stage::Features, 0.0);

        let rows = features::table(&bars);

        // The return from each bar's close to the close `horizon` later.
        let actual: Vec<Option<f64>> = (0..count)
            .map(|at| {
                (at + horizon < count).then(|| (bars[at + horizon].close / bars[at].close).ln())
            })
            .collect();

        // Training targets must be known before the held-out part
        // begins: bar `at` is fitted only if `at + horizon` is still in
        // the training part.
        let known = first_test - horizon;
        let first_checked = known - (known as f64 * CHECKED) as usize;

        let scaler = Scaler::fit(
            rows[..first_test].iter().map(|row| row.as_slice()),
            FEATURES,
        );

        let targets_known: Vec<f64> = actual[..known].iter().flatten().copied().collect();
        let target_mean = targets_known.iter().sum::<f64>() / known as f64;
        let target_spread = {
            let squares: f64 = targets_known
                .iter()
                .map(|x| (x - target_mean).powi(2))
                .sum();
            let spread = (squares / known as f64).sqrt();

            if spread > 0.0 { spread } else { 1.0 }
        };

        let gaps_ms: Vec<f64> = bars[..first_test]
            .windows(2)
            .map(|two| (two[1].time_ms - two[0].time_ms) as f64)
            .collect();

        let gap_ms = stats::quantile(&gaps_ms, 0.5).max(1.0);
        let gaps: Vec<f32> = (0..count).map(|at| gap(&bars, at, gap_ms)).collect();

        let features_ms = began.elapsed().as_secs_f64() * 1e3;

        // The dendritic network lives the bars in order. What it says
        // at each is from what it has lived up to then.
        let dendritic_began = Instant::now();

        let mut regime = Regime::new(Buckets::fit(&rows[..first_test]));
        let mut outlooks = Vec::with_capacity(count);

        for (at, row) in rows.iter().enumerate() {
            if at % 32 == 0 {
                tell(Stage::Dendritic, at as f32 / count as f32);
            }

            outlooks.push(regime.observe(row));
        }

        let outlook_rows: Vec<[f64; OUTLOOK]> = outlooks.iter().map(Outlook::row).collect();

        let outlook_scaler = Scaler::fit(
            outlook_rows[..first_test].iter().map(|row| row.as_slice()),
            OUTLOOK,
        );

        let dendritic_ms = dendritic_began.elapsed().as_secs_f64() * 1e3;

        // The two lessons differ only in what is shown.
        let training_began = Instant::now();

        let with_outlook: Vec<f32> = (0..count)
            .flat_map(|at| inputs(&scaler, &outlook_scaler, &rows[at], &outlooks[at]))
            .collect();

        let without: Vec<f32> = with_outlook
            .chunks(FEATURES + OUTLOOK)
            .flat_map(|row| row[..FEATURES].iter().copied())
            .collect();

        let mut targets = vec![0.0f32; count * 2];
        let mut fit = vec![0.0f32; count * 2];
        let mut check = vec![0.0f32; count * 2];

        for at in 0..known {
            let Some(actual) = actual[at] else {
                continue;
            };

            let scaled = (actual - target_mean) / target_spread;

            targets[at * 2] = scaled.clamp(-FURTHEST_TARGET, FURTHEST_TARGET) as f32;
            targets[at * 2 + 1] = (actual > 0.0) as u8 as f32;

            let weights = if at >= first_checked {
                &mut check
            } else if at >= WARMUP {
                &mut fit
            } else {
                continue;
            };

            weights[at * 2] = 1.0;

            // A bar that did not move has no direction to learn.
            weights[at * 2 + 1] = (actual != 0.0) as u8 as f32;
        }

        let training = Training {
            epochs: settings.epochs,
            window: settings.window,
            seed: settings.seed,
            ..Training::default()
        };

        let learn = |shown: &[f32], width: usize, stage: Stage| {
            let shape = Shape {
                inputs: width,
                units: settings.units,
                backbone: settings.units,
                heads: vec![Head::Number, Head::Odds],
            };

            let lesson = Lesson {
                inputs: shown,
                gaps: &gaps,
                targets: &targets,
                fit: &fit,
                check: &check,
            };

            let mut cell = Cfc::new(shape, settings.seed);

            tell(stage, 0.0);

            let losses = cell.train(&lesson, &training, &mut |epoch, _| {
                tell(stage, (epoch + 1) as f32 / settings.epochs as f32);
            });

            let mut state = cell.fresh_state();
            let outs = cell.run(&mut state, shown, &gaps);

            (cell, state, outs, losses)
        };

        let (_, _, liquid_outs, liquid_losses) = learn(&without, FEATURES, Stage::Liquid);

        let (cell, state, combined_outs, combined_losses) =
            learn(&with_outlook, FEATURES + OUTLOOK, Stage::Combined);

        let training_ms = training_began.elapsed().as_secs_f64() * 1e3;

        tell(Stage::Scoring, 0.0);

        let expected = |outs: &[f32], at: usize| outs[at * 2] as f64 * target_spread + target_mean;
        let called = |outs: &[f32], at: usize| {
            let odds = odds(outs[at * 2 + 1]);

            (odds != 0.5).then_some(odds > 0.5)
        };

        // The held-out bars whose outcome is known.
        let scored: Vec<usize> = (first_test..count - horizon).collect();
        let actual_at = |at: usize| actual[at].unwrap_or_default();

        let buckets = regime.buckets().clone();

        let judge = |predictor: Predictor,
                     call: &dyn Fn(usize) -> Option<bool>,
                     predicted: &dyn Fn(usize) -> f64| {
            score(
                predictor,
                horizon,
                scored
                    .iter()
                    .map(|at| (call(*at), predicted(*at), actual_at(*at))),
            )
        };

        // The return over the `horizon` bars just gone.
        let last = |at: usize| (bars[at].close / bars[at - horizon].close).ln();

        let scores = vec![
            judge(Predictor::Liquid, &|at| called(&liquid_outs, at), &|at| {
                expected(&liquid_outs, at)
            }),
            judge(Predictor::Dendritic, &|at| outlooks[at].up(), &|at| {
                outlooks[at].surest().map_or(0.0, |(bucket, sure)| {
                    sure as f64 * buckets.move_means[bucket] * horizon as f64
                })
            }),
            judge(
                Predictor::Combined,
                &|at| called(&combined_outs, at),
                &|at| expected(&combined_outs, at),
            ),
            judge(Predictor::AlwaysUp, &|_| Some(true), &|_| target_mean),
            judge(
                Predictor::Persistence,
                &|at| {
                    let last = last(at);

                    (last != 0.0).then_some(last > 0.0)
                },
                &last,
            ),
            judge(Predictor::Zero, &|_| None, &|_| 0.0),
        ];

        let verdict = verdict(&scores, horizon);

        let series = Series {
            first_bar: first_test,
            predicted_return: (first_test..count)
                .map(|at| expected(&combined_outs, at) as f32)
                .collect(),
            probability_up: (first_test..count)
                .map(|at| odds(combined_outs[at * 2 + 1]))
                .collect(),
            actual_return: (first_test..count)
                .map(|at| actual[at].map(|x| x as f32))
                .collect(),
        };

        let mut notes = Vec::new();

        if dropped > 0 {
            notes.push(format!(
                "{dropped} bars had the same time as another and were dropped."
            ));
        }

        let spoke = scored
            .iter()
            .filter(|at| outlooks[**at].up().is_some())
            .count();

        notes.push(format!(
            "The dendritic network expected a direction on {spoke} of the {} held-out bars; its \
             accuracy is over those only. It went on learning as the held-out bars passed, from \
             bars already closed.",
            scored.len()
        ));

        if horizon > 1 {
            notes.push(format!(
                "Returns over {horizon} bars overlap from one bar to the next, so the intervals \
                 and p-values count every {horizon} scored bars as one."
            ));
        }

        if buckets.volume.is_none() {
            notes.push("The bars have no volume, so nothing was made of it.".to_string());
        }

        notes.push(
            "Measured on past bars with no trading costs. A forecast, not advice.".to_string(),
        );

        let mut model = Model {
            settings: settings.clone(),
            scaler,
            outlook_scaler,
            target_mean,
            target_spread,
            gap_ms,
            cell,
            state,
            recent: bars[count.saturating_sub(KEPT)..].to_vec(),
            forecast: Forecast {
                after_time_ms: 0,
                horizon,
                expected_return: 0.0,
                direction: Direction::Flat,
                confidence: 0.5,
                probability_up: 0.5,
                dendritic: Account::default(),
            },
            regime,
        };

        model.refresh(&combined_outs[(count - 1) * 2..]);

        let analysis = Analysis {
            bars: count,
            train_bars: first_test,
            horizon,
            scores,
            verdict,
            forecast: model.forecast.clone(),
            series,
            liquid_losses,
            combined_losses,
            regimes: model.regime.lived().iter().map(regime::regime_of).collect(),
            regime_names: regime::regime_names(),
            growth: model.regime.growth(),
            notes,
            timings: Timings {
                features_ms,
                dendritic_ms,
                training_ms,
                total_ms: began.elapsed().as_secs_f64() * 1e3,
            },
        };

        tell(Stage::Done, 1.0);

        Ok((model, analysis))
    }

    /// Rewrites the forecast from the network's outputs at the latest
    /// bar.
    fn refresh(&mut self, out: &[f32]) {
        let probability_up = odds(out[1]);

        self.forecast = Forecast {
            after_time_ms: self.recent.last().map_or(0, |bar| bar.time_ms),
            horizon: self.settings.horizon,
            expected_return: out[0] as f64 * self.target_spread + self.target_mean,
            direction: if probability_up > 0.5 {
                Direction::Up
            } else if probability_up < 0.5 {
                Direction::Down
            } else {
                Direction::Flat
            },
            confidence: probability_up.max(1.0 - probability_up),
            probability_up,
            dendritic: self.regime.account(),
        };
    }

    /// Takes bars that came after the ones it has seen: both networks
    /// are run over them and the forecast is rewritten. The dendritic
    /// network learns from them as it always does; the liquid network's
    /// weights do not change. Train again for that.
    ///
    /// Bars not later than the latest seen, and bars that cannot be
    /// used, are left out and counted.
    pub fn update(&mut self, new_bars: &[Bar]) -> Update {
        let (new_bars, _) = bars::tidy(new_bars.to_vec());

        let mut update = Update {
            added: 0,
            ignored: 0,
        };

        let mut out = None;

        for bar in new_bars {
            let latest = self.recent.last().map_or(i64::MIN, |bar| bar.time_ms);

            if bar.time_ms <= latest || bar.problem().is_some() {
                update.ignored += 1;

                continue;
            }

            self.recent.push(bar);

            if self.recent.len() > KEPT {
                self.recent.remove(0);
            }

            let at = self.recent.len() - 1;
            let row = features::row(&self.recent, at);
            let outlook = self.regime.observe(&row);

            let shown = inputs(&self.scaler, &self.outlook_scaler, &row, &outlook);
            let gap = gap(&self.recent, at, self.gap_ms);

            out = Some(self.cell.run(&mut self.state, &shown, &[gap]));
            update.added += 1;
        }

        if let Some(out) = out {
            self.refresh(&out);
        }

        update
    }

    /// What it makes of the bars after the latest one it has seen.
    pub fn forecast(&self) -> &Forecast {
        &self.forecast
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Bars seen, in training and since.
    pub fn bars_seen(&self) -> usize {
        self.regime.lived().len()
    }

    /// How the dendritic network has grown so far.
    pub fn growth(&self) -> Growth {
        self.regime.growth()
    }

    /// The dendritic network's own account of what comes next.
    pub fn account(&self) -> Account {
        self.regime.account()
    }

    /// The bucket edges the bars are put into words by.
    pub fn regime_buckets(&self) -> &Buckets {
        self.regime.buckets()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Reads a model written by [`Model::to_json`]. The dendritic
    /// network is rebuilt by living its bars again, which takes about a
    /// twentieth of a millisecond per bar on a laptop.
    pub fn from_json(text: &str) -> Result<Self, Error> {
        let model: Model =
            serde_json::from_str(text).map_err(|error| Error::Saved(error.to_string()))?;

        let shape = model.cell.shape();

        let fits = model.cell.is_sound()
            && shape.inputs == FEATURES + OUTLOOK
            && shape.heads.len() == 2
            && model.state.len() == shape.units
            && model.scaler.mean.len() == FEATURES
            && model.scaler.spread.len() == FEATURES
            && model.outlook_scaler.mean.len() == OUTLOOK
            && model.outlook_scaler.spread.len() == OUTLOOK
            && model.gap_ms > 0.0
            && !model.recent.is_empty();

        if !fits {
            return Err(Error::Saved(
                "its parts do not fit together; it may be cut short or from another version"
                    .to_string(),
            ));
        }

        Ok(model)
    }
}

/// Trains on `bars` and measures the result on their last part. See the
/// module's own description for how, and [`Model::train`] to keep the
/// trained model as well.
///
/// Does not touch the screen, the disk or the network, so it can be
/// called from any thread.
pub fn analyse(
    bars: &[Bar],
    settings: &Settings,
    progress: &dyn Fn(Progress),
) -> Result<Analysis, Error> {
    Model::train(bars, settings, progress).map(|(_, analysis)| analysis)
}

/// Scores one predictor from, for each held-out bar, the direction it
/// called (if any), the return it predicted and the return that came.
fn score(
    predictor: Predictor,
    horizon: usize,
    bars: impl Iterator<Item = (Option<bool>, f64, f64)>,
) -> Score {
    let (mut count, mut scored, mut right, mut off) = (0, 0, 0, 0.0);

    for (call, predicted, actual) in bars {
        count += 1;
        off += (predicted - actual).abs();

        // A bar that did not move was neither up nor down.
        if let (Some(up), true) = (call, actual != 0.0) {
            scored += 1;
            right += (up == (actual > 0.0)) as usize;
        }
    }

    let (apart_right, apart) = apart(right, scored, horizon);
    let interval = (scored > 0).then(|| stats::wilson(apart_right, apart, Z_99));

    Score {
        predictor,
        name: predictor.name().to_string(),
        bars: count,
        scored,
        right,
        accuracy: (scored > 0).then(|| right as f64 / scored as f64),
        accuracy_low: interval.map(|(low, _)| low),
        accuracy_high: interval.map(|(_, high)| high),
        p_value: (scored > 0).then(|| stats::binomial_two_sided(apart_right, apart)),
        mean_abs_error: if count > 0 { off / count as f64 } else { 0.0 },
    }
}

/// The counts as if only every `horizon`-th scored bar had been counted,
/// since returns over more than one bar overlap and are not separate
/// trials.
fn apart(right: usize, scored: usize, horizon: usize) -> (u64, u64) {
    if horizon <= 1 || scored == 0 {
        return (right as u64, scored as u64);
    }

    let apart = (scored / horizon).max(1) as u64;
    let share = right as f64 / scored as f64;

    ((share * apart as f64).round() as u64, apart)
}

fn verdict(scores: &[Score], horizon: usize) -> Verdict {
    let find = |predictor: Predictor| scores.iter().find(|score| score.predictor == predictor);

    let (best_baseline, best) = [Predictor::AlwaysUp, Predictor::Persistence]
        .into_iter()
        .filter_map(|predictor| Some((predictor, find(predictor)?.accuracy?)))
        .max_by(|left, right| left.1.total_cmp(&right.1))
        .unwrap_or((Predictor::AlwaysUp, 0.5));

    let accuracy_to_beat = best.max(0.5);

    let percent = |share: f64| format!("{:.1}%", share * 100.0);
    let against = format!(
        "the best simple baseline ({}) was right {}",
        best_baseline.name().trim_start_matches("baseline: "),
        percent(best)
    );

    let closing = "This is a measurement on past bars, not advice.";

    let combined = find(Predictor::Combined).filter(|score| score.scored > 0);

    let Some((combined, accuracy)) = combined.and_then(|score| Some((score, score.accuracy?)))
    else {
        return Verdict {
            edge: Edge::None,
            text: format!("No edge found: no held-out bar could be scored. {closing}"),
            best_baseline,
            accuracy_to_beat,
        };
    };

    let (right, of) = apart(combined.right, combined.scored, horizon);

    let (edge, text) = if of < FEWEST_FOR_VERDICT {
        (
            Edge::None,
            format!(
                "No edge found: only {of} independent held-out bars could be scored, too few to \
                 tell anything from chance."
            ),
        )
    } else if stats::wilson(right, of, Z_999).0 > accuracy_to_beat {
        (
            Edge::Present,
            format!(
                "An edge on the held-out bars: the combined model called the direction right {} \
                 of the time over {} bars, and {against}. The gap is more than a 99.9% interval \
                 allows for chance. It may not last, and trading costs are not counted.",
                percent(accuracy),
                combined.scored
            ),
        )
    } else if stats::wilson(right, of, Z_99).0 > accuracy_to_beat {
        (
            Edge::Weak,
            format!(
                "A weak edge on the held-out bars: the combined model called the direction right \
                 {} of the time over {} bars, and {against}. The gap is more than a 99% interval \
                 allows for chance, which chance alone still manages about once in two hundred \
                 runs. Trading costs are not counted.",
                percent(accuracy),
                combined.scored
            ),
        )
    } else {
        (
            Edge::None,
            format!(
                "No edge found. The combined model called the direction right {} of the time \
                 over {} held-out bars, and {against}. That is within what chance gives, which \
                 is the usual result: prices are close to a random walk.",
                percent(accuracy),
                combined.scored
            ),
        )
    };

    Verdict {
        edge,
        text: format!("{text} {closing}"),
        best_baseline,
        accuracy_to_beat,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scored(predictor: Predictor, right: usize, of: usize) -> Score {
        score(predictor, 1, (0..of).map(|at| (Some(at < right), 0.0, 1.0)))
    }

    fn verdict_for(combined: (usize, usize), up: (usize, usize), same: (usize, usize)) -> Verdict {
        verdict(
            &[
                scored(Predictor::Combined, combined.0, combined.1),
                scored(Predictor::AlwaysUp, up.0, up.1),
                scored(Predictor::Persistence, same.0, same.1),
            ],
            1,
        )
    }

    #[test]
    fn a_score_counts_only_called_bars_that_moved() {
        let score = score(
            Predictor::Liquid,
            1,
            [
                (Some(true), 0.02, 0.01),
                (Some(true), 0.02, -0.01),
                (Some(false), -0.01, -0.03),
                // No call, and a bar that did not move.
                (None, 0.0, 0.05),
                (Some(true), 0.01, 0.0),
            ]
            .into_iter(),
        );

        assert_eq!((score.bars, score.scored, score.right), (5, 3, 2));
        assert!((score.accuracy.unwrap() - 2.0 / 3.0).abs() < 1e-12);
        assert!((score.mean_abs_error - (0.01 + 0.03 + 0.02 + 0.05 + 0.01) / 5.0).abs() < 1e-12);
        assert!((score.p_value.unwrap() - 1.0).abs() < 1e-9);
        assert!(score.accuracy_low.unwrap() < 0.5 && score.accuracy_high.unwrap() > 0.7);

        let silent = super::score(Predictor::Zero, 1, [(None, 0.0, 0.01)].into_iter());

        assert_eq!(silent.accuracy, None);
        assert_eq!(silent.p_value, None);
    }

    #[test]
    fn the_verdict_is_none_unless_chance_and_every_baseline_are_beaten() {
        // 52% of 1,000 is within chance.
        assert_eq!(
            verdict_for((520, 1_000), (500, 1_000), (480, 1_000)).edge,
            Edge::None
        );

        // 56% of 1,000 beats a coin by the 99.9% interval.
        let found = verdict_for((560, 1_000), (500, 1_000), (480, 1_000));

        assert_eq!(found.edge, Edge::Present);
        assert_eq!(found.accuracy_to_beat, 0.5);

        // Between the two intervals.
        assert_eq!(
            verdict_for((545, 1_000), (500, 1_000), (480, 1_000)).edge,
            Edge::Weak
        );

        // The same 56% where the price simply rose 55% of the time.
        let drift = verdict_for((560, 1_000), (550, 1_000), (480, 1_000));

        assert_eq!(drift.edge, Edge::None);
        assert_eq!(drift.best_baseline, Predictor::AlwaysUp);
        assert!(drift.text.starts_with("No edge found"), "{}", drift.text);

        // A high share of very few bars proves nothing.
        assert_eq!(verdict_for((18, 20), (10, 20), (10, 20)).edge, Edge::None);

        // Being reliably wrong is not an edge either.
        assert_eq!(
            verdict_for((300, 1_000), (500, 1_000), (500, 1_000)).edge,
            Edge::None
        );

        for verdict in [found, drift] {
            assert!(verdict.text.ends_with("not advice."), "{}", verdict.text);
        }
    }

    #[test]
    fn overlapping_returns_count_for_less() {
        assert_eq!(apart(60, 100, 1), (60, 100));
        assert_eq!(apart(60, 100, 5), (12, 20));
        assert_eq!(apart(3, 3, 5), (1, 1));

        // 60% of 400 is an edge one bar ahead and not ten ahead.
        let one = verdict(
            &[
                scored(Predictor::Combined, 240, 400),
                scored(Predictor::AlwaysUp, 200, 400),
            ],
            1,
        );
        let ten = verdict(
            &[
                scored(Predictor::Combined, 240, 400),
                scored(Predictor::AlwaysUp, 200, 400),
            ],
            10,
        );

        assert_eq!(one.edge, Edge::Present);
        assert_eq!(ten.edge, Edge::None);
    }

    #[test]
    fn settings_out_of_range_are_refused() {
        let refused = |settings: Settings| {
            matches!(
                Model::train(&[], &settings, &|_| {}),
                Err(Error::BadSettings(_))
            )
        };

        assert!(refused(Settings {
            horizon: 0,
            ..Settings::default()
        }));
        assert!(refused(Settings {
            units: 1,
            ..Settings::default()
        }));
        assert!(refused(Settings {
            test_share: 0.9,
            ..Settings::default()
        }));
        assert!(refused(Settings {
            epochs: 0,
            ..Settings::default()
        }));

        assert_eq!(Settings::default().fewest_bars(), 85);
    }
}
