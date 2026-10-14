//! The phones' trading screen: an instrument live from Dhan, or bars from
//! a file or a chart picture, forecast by the liquid, dendritic or
//! spiking network or the phone's chat model.
//!
//! The work is `cortex-market`'s; this hands its snapshot over in the
//! plain shapes Kotlin and Swift take.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cortex_market::market::time_label;
use cortex_market::{
    Asking, Brain, Connection, FeedState, Instrument, Interval, Market, ProviderOracle, Snapshot,
    Source, Tone, Work, clock,
};

use crate::lock;

/// Told when there is something new to fetch. Called from a background
/// thread.
#[uniffi::export(with_foreign)]
pub trait MarketListener: Send + Sync {
    fn changed(&self);
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct InstrumentView {
    /// Identifies it: `NSE_EQ:2885`.
    pub key: String,
    pub symbol: String,
    pub name: String,
    pub exchange: String,
}

/// A coloured line of status: `good`, `bad`, `warn`, `accent` or `muted`.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct StatusView {
    pub text: String,
    pub tone: String,

    /// From 0 to 1 while training.
    pub fraction: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct NowView {
    pub price: f64,
    pub change: Option<f64>,
    pub change_pct: Option<f64>,
    pub vwap: Option<f64>,
    pub rsi: Option<f64>,
    pub ema_fast: Option<f64>,
    pub ema_slow: Option<f64>,
    pub atr: Option<f64>,
    pub volume: Option<f64>,
    pub day_high: Option<f64>,
    pub day_low: Option<f64>,
    pub bid: Option<f64>,
    pub ask: Option<f64>,
    pub spread: Option<f64>,
    pub bid_quantity: Option<f64>,
    pub ask_quantity: Option<f64>,
    pub buy_share: Option<f64>,
    pub open_interest: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ForecastRowView {
    pub step: u32,
    pub time: String,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
    pub close_low: f64,
    pub close_high: f64,
    pub change_pct: f64,
    pub rsi: Option<f64>,
    pub vwap: Option<f64>,
    pub turnover: f64,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ScenarioView {
    /// `up`, `down` or `neutral`.
    pub tone: String,
    pub case: String,
    pub expect: String,
    pub action: String,
    pub entry: Option<f64>,
    pub target: Option<f64>,
    pub stop: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PlanView {
    pub bias: String,
    pub tone: String,
    pub score: i32,
    pub reasons: Vec<String>,
    pub support: f64,
    pub resistance: f64,
    pub scenarios: Vec<ScenarioView>,
}

/// Everything the screen shows at the top and on the forecast tab.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct MarketView {
    pub revision: u64,

    /// `off`, `checking`, `connected` or `failed`, and what to say.
    pub connection: String,
    pub connection_detail: String,

    /// Empty when nothing is chosen.
    pub title: String,

    /// `dhan`, `file`, `picture` or empty.
    pub source: String,
    pub interval: String,

    /// The brain in use: short name, name, and what it is.
    pub brain: String,
    pub brain_name: String,
    pub brain_about: String,

    /// The phone's chat model, if one is set up.
    pub llm_model: Option<String>,
    pub llm_asking: bool,
    pub llm_problem: Option<String>,
    pub llm_reason: String,

    pub status: Option<StatusView>,
    pub problem: Option<String>,
    pub notes: Vec<String>,

    pub now: NowView,
    pub rows: Vec<ForecastRowView>,
    pub plan: Option<PlanView>,

    /// The brain's measured share right on held-out bars at +1, +5 and
    /// +10, and live.
    pub held_out: Vec<Option<f64>>,
    pub live_right: u32,
    pub live_judged: u32,

    pub has_instrument_list: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct ChartCandle {
    pub time_ms: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
    pub vwap: Option<f64>,
    pub ema_fast: Option<f64>,
    pub ema_slow: Option<f64>,
    pub rsi: Option<f64>,
    pub forming: bool,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ChartView {
    pub candles: Vec<ChartCandle>,
    pub support: Option<f64>,
    pub resistance: Option<f64>,
    pub intraday: bool,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TallyView {
    pub brain: String,
    pub name: String,
    pub selected: bool,

    /// Share right and how many judged, at +1, +5 and +10.
    pub right: Vec<Option<f64>>,
    pub judged: Vec<u32>,
    pub inside: Option<f64>,
    pub error_pct: Option<f64>,
    pub waiting: u32,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct OutcomeView {
    pub made: String,
    pub brain: String,
    pub step: u32,
    pub due: String,
    pub from: f64,
    pub forecast: f64,
    pub actual: Option<f64>,
    pub error_pct: Option<f64>,
    pub inside: Option<bool>,

    /// `waiting`, `right`, `wrong` or `flat`.
    pub result: String,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct OutcomesView {
    pub live: bool,
    pub tallies: Vec<TallyView>,
    pub outcomes: Vec<OutcomeView>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct StepView {
    pub step: u32,
    pub accuracy: Option<f64>,
    pub low: Option<f64>,
    pub high: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FactView {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct NetMetricsView {
    pub brain: String,
    pub name: String,
    pub about: String,
    pub selected: bool,
    pub steps: Vec<StepView>,
    pub error_pct: Option<f64>,
    pub skill: Option<f64>,
    pub band_hit: Option<f64>,
    pub volume_error: Option<f64>,
    pub parameters: u64,
    pub neurons: u64,
    pub weight: f32,
    pub train_ms: f64,
    pub facts: Vec<FactView>,
    pub loss_fit: Vec<f32>,
    pub loss_check: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct MetricsView {
    pub trained: bool,
    pub trained_on: String,
    pub bars: u32,
    pub held_out: u32,
    pub total_ms: f64,
    pub nets: Vec<NetMetricsView>,
    pub baseline: Vec<StepView>,
    pub llm_model: Option<String>,
    pub llm_calls: u32,
    pub llm_failures: u32,
    pub llm_mean_ms: f64,
    pub llm_live_right: u32,
    pub llm_live_judged: u32,
}

#[derive(uniffi::Object)]
pub struct MarketDesk {
    market: Market,
    app_folder: PathBuf,

    /// Instruments the screen has been shown, by key.
    known: Mutex<HashMap<String, Instrument>>,
}

fn tone_text(tone: Tone) -> String {
    match tone {
        Tone::Up => "up",
        Tone::Down => "down",
        Tone::Neutral => "neutral",
    }
    .to_string()
}

fn status(snapshot: &Snapshot) -> Option<StatusView> {
    let view = |text: String, tone: &str, fraction: Option<f32>| {
        Some(StatusView {
            text,
            tone: tone.to_string(),
            fraction,
        })
    };

    match (&snapshot.work, &snapshot.feed) {
        (Work::Loading(what), _) => view(format!("Loading {what}"), "muted", None),
        (Work::Training(fraction), _) => view(
            format!("Training {:.0}%", fraction * 100.0),
            "accent",
            Some(*fraction),
        ),
        (Work::Failed(reason), _) => view(reason.clone(), "bad", None),
        (_, FeedState::Live) => view("Live".to_string(), "good", None),
        (_, FeedState::Connecting) => view("Connecting".to_string(), "muted", None),
        (_, FeedState::Retrying(_)) => view("Reconnecting".to_string(), "warn", None),
        (_, FeedState::Stopped(reason)) => view(reason.clone(), "bad", None),
        _ => match &snapshot.source {
            Some(Source::Dhan { .. }) => view("Market closed".to_string(), "muted", None),
            Some(_) => view("Offline".to_string(), "muted", None),
            None => None,
        },
    }
}

#[uniffi::export]
impl MarketDesk {
    /// `folder` is the app's own folder; the market keeps its files in
    /// `trading` under it.
    #[uniffi::constructor]
    pub fn new(folder: String, listener: Arc<dyn MarketListener>) -> Arc<Self> {
        let app_folder = PathBuf::from(&folder);
        let market = Market::new(&app_folder.join("trading"), move || listener.changed());

        let desk = Arc::new(Self {
            market,
            app_folder,
            known: Mutex::default(),
        });

        desk.use_phone_model();

        desk
    }

    /// Sets the language-model forecast up with the phone's chat model.
    /// Call again after that model is changed.
    pub fn use_phone_model(&self) {
        let provider = crate::local::phone_provider(&self.app_folder).ok();

        self.market.set_oracle(provider.map(|provider| {
            Arc::new(ProviderOracle::new(provider)) as Arc<dyn cortex_market::Oracle>
        }));
    }

    pub fn connect(&self, client_id: String, access_token: String) {
        self.market.connect(&client_id, &access_token);
    }

    pub fn disconnect(&self) {
        self.market.disconnect();
    }

    /// The saved client ID, to fill the form with.
    pub fn client_id(&self) -> Option<String> {
        self.market.client_id()
    }

    pub fn search(&self, query: String) -> Vec<InstrumentView> {
        let found = self.market.search(&query, 40);
        let mut known = lock(&self.known);

        found
            .into_iter()
            .map(|instrument| {
                let view = InstrumentView {
                    key: instrument.key(),
                    symbol: instrument.symbol.clone(),
                    name: instrument.name.clone(),
                    exchange: instrument.segment.exchange().to_string(),
                };

                known.insert(view.key.clone(), instrument);

                view
            })
            .collect()
    }

    pub fn fetch_instrument_list(&self) {
        self.market.fetch_instrument_list();
    }

    /// Watches an instrument returned by [`MarketDesk::search`].
    pub fn watch(&self, key: String) {
        let instrument = lock(&self.known).get(&key).cloned();

        if let Some(instrument) = instrument {
            self.market.watch(instrument);
        }
    }

    pub fn refresh(&self) {
        self.market.refresh();
    }

    /// `LNN`, `DANN`, `SNN`, `ENS` or `LLM`.
    pub fn set_brain(&self, brain: String) {
        if let Some(brain) = Brain::from_short(&brain) {
            self.market.set_brain(brain);
        }
    }

    /// `1m`, `5m`, `15m`, `25m`, `1h` or `1D`.
    pub fn set_interval(&self, interval: String) {
        if let Some(interval) = Interval::from_label(&interval) {
            self.market.set_interval(interval);
        }
    }

    /// Reads a CSV file's text. Returns why it could not, if it could not.
    pub fn load_csv(&self, name: String, text: String) -> Option<String> {
        self.market.load_csv(&name, &text).err()
    }

    /// Reads candles off a chart picture. `top` and `bottom` are the
    /// highest and lowest prices on it, if known.
    pub fn load_picture(
        &self,
        name: String,
        bytes: Vec<u8>,
        top: Option<f64>,
        bottom: Option<f64>,
    ) -> Option<String> {
        let prices = match (top, bottom) {
            (Some(top), Some(bottom)) if top > bottom && bottom > 0.0 => Some((top, bottom)),
            _ => None,
        };

        self.market.load_picture(&name, bytes, prices).err()
    }

    pub fn ask_llm(&self) {
        self.market.ask_llm(true);
    }

    pub fn clear_outcomes(&self) {
        self.market.clear_outcomes(false);
    }

    pub fn revision(&self) -> u64 {
        self.market.revision()
    }

    pub fn view(&self) -> MarketView {
        let snapshot = self.market.snapshot();
        let intraday = snapshot.interval.is_intraday();
        let brain = snapshot.brain;

        let (connection, connection_detail) = match &snapshot.connection {
            Connection::Off => ("off", String::new()),
            Connection::Checking => ("checking", String::new()),
            Connection::Connected(profile) => ("connected", profile.client_id.clone()),
            Connection::Failed(reason) => ("failed", reason.clone()),
        };

        let now = &snapshot.now;

        let held_out = [1, 5, 10]
            .into_iter()
            .map(|step| {
                brain
                    .net()
                    .and_then(|net| snapshot.report.as_ref()?.of(net)?.at(step)?.accuracy)
            })
            .collect();

        let mine = snapshot
            .tallies
            .iter()
            .filter(|tally| tally.source == brain.short());
        let (live_right, live_judged) = mine.fold((0, 0), |(right, judged), tally| {
            (right + tally.right as u32, judged + tally.judged as u32)
        });

        MarketView {
            revision: snapshot.revision,
            connection: connection.to_string(),
            connection_detail,
            title: snapshot
                .source
                .as_ref()
                .map(Source::title)
                .unwrap_or_default(),
            source: match &snapshot.source {
                Some(Source::Dhan { .. }) => "dhan",
                Some(Source::File { .. }) => "file",
                Some(Source::Picture { .. }) => "picture",
                None => "",
            }
            .to_string(),
            interval: snapshot.interval.label().to_string(),
            brain: brain.short().to_string(),
            brain_name: brain.name().to_string(),
            brain_about: brain.about().to_string(),
            llm_model: snapshot.llm.model.clone(),
            llm_asking: snapshot.llm.asking == Asking::Asking,
            llm_problem: match &snapshot.llm.asking {
                Asking::Failed(reason) => Some(reason.clone()),
                _ => None,
            },
            llm_reason: snapshot.llm.reason.clone(),
            status: status(&snapshot),
            problem: snapshot.problem.clone(),
            notes: snapshot.notes.clone(),
            now: NowView {
                price: now.price,
                change: now.change,
                change_pct: now.change_pct,
                vwap: now.vwap,
                rsi: now.rsi,
                ema_fast: now.ema_fast,
                ema_slow: now.ema_slow,
                atr: now.atr,
                volume: now.volume,
                day_high: now.day_high,
                day_low: now.day_low,
                bid: now.bid,
                ask: now.ask,
                spread: now.spread,
                bid_quantity: now.bid_quantity,
                ask_quantity: now.ask_quantity,
                buy_share: now.buy_share,
                open_interest: now.open_interest,
            },
            rows: snapshot
                .rows
                .iter()
                .map(|row| ForecastRowView {
                    step: row.step as u32,
                    time: time_label(row.time_ms, intraday),
                    open: row.open,
                    high: row.high,
                    low: row.low,
                    close: row.close,
                    volume: row.volume,
                    close_low: row.close_low,
                    close_high: row.close_high,
                    change_pct: row.change_pct,
                    rsi: row.rsi,
                    vwap: row.vwap,
                    turnover: row.turnover,
                })
                .collect(),
            plan: snapshot.plan.as_ref().map(|plan| PlanView {
                bias: plan.bias.label.clone(),
                tone: tone_text(plan.bias.tone),
                score: plan.bias.score,
                reasons: plan.bias.reasons.clone(),
                support: plan.support,
                resistance: plan.resistance,
                scenarios: plan
                    .scenarios
                    .iter()
                    .map(|scenario| ScenarioView {
                        tone: tone_text(scenario.tone),
                        case: scenario.case.clone(),
                        expect: scenario.expect.clone(),
                        action: scenario.action.clone(),
                        entry: scenario.entry,
                        target: scenario.target,
                        stop: scenario.stop,
                    })
                    .collect(),
            }),
            held_out,
            live_right,
            live_judged,
            has_instrument_list: self.market.has_instrument_list(),
        }
    }

    /// The latest `count` candles (the forming one last), with their
    /// indicators, and the forecast after them is in [`MarketView::rows`].
    pub fn chart(&self, count: u32) -> ChartView {
        let snapshot = self.market.snapshot();
        let bars = &snapshot.bars;
        let indicators = &snapshot.indicators;
        let first = bars.len().saturating_sub(count as usize);

        let mut candles: Vec<ChartCandle> = (first..bars.len())
            .map(|at| {
                let bar = bars[at];
                let value = |values: &Vec<Option<f64>>| values.get(at).copied().flatten();

                ChartCandle {
                    time_ms: bar.time_ms,
                    open: bar.open,
                    high: bar.high,
                    low: bar.low,
                    close: bar.close,
                    volume: bar.volume,
                    vwap: value(&indicators.vwap),
                    ema_fast: value(&indicators.ema_fast),
                    ema_slow: value(&indicators.ema_slow),
                    rsi: value(&indicators.rsi),
                    forming: false,
                }
            })
            .collect();

        if let Some(bar) = snapshot.forming {
            candles.push(ChartCandle {
                time_ms: bar.time_ms,
                open: bar.open,
                high: bar.high,
                low: bar.low,
                close: bar.close,
                volume: bar.volume,
                vwap: None,
                ema_fast: None,
                ema_slow: None,
                rsi: None,
                forming: true,
            });
        }

        ChartView {
            candles,
            support: snapshot.plan.as_ref().map(|plan| plan.support),
            resistance: snapshot.plan.as_ref().map(|plan| plan.resistance),
            intraday: snapshot.interval.is_intraday(),
        }
    }

    pub fn outcomes(&self) -> OutcomesView {
        let snapshot = self.market.snapshot();
        let intraday = snapshot.interval.is_intraday();

        let tallies = Brain::ALL
            .into_iter()
            .filter_map(|brain| {
                let of = |step: usize| {
                    snapshot
                        .tallies
                        .iter()
                        .find(|tally| tally.source == brain.short() && tally.step == step)
                };

                let made: usize = snapshot
                    .tallies
                    .iter()
                    .filter(|tally| tally.source == brain.short())
                    .map(|tally| tally.made)
                    .sum();

                if made == 0 {
                    return None;
                }

                let waiting: usize = snapshot
                    .tallies
                    .iter()
                    .filter(|tally| tally.source == brain.short())
                    .map(|tally| tally.made - tally.resolved)
                    .sum();

                Some(TallyView {
                    brain: brain.short().to_string(),
                    name: brain.name().to_string(),
                    selected: brain == snapshot.brain,
                    right: [1, 5, 10]
                        .map(|step| of(step).and_then(|tally| tally.accuracy()))
                        .to_vec(),
                    judged: [1, 5, 10]
                        .map(|step| of(step).map_or(0, |tally| tally.judged as u32))
                        .to_vec(),
                    inside: of(1).and_then(|tally| tally.inside_rate()),
                    error_pct: of(1)
                        .filter(|tally| tally.resolved > 0)
                        .map(|tally| tally.mean_error_pct),
                    waiting: waiting as u32,
                })
            })
            .collect();

        let outcomes = snapshot
            .outcomes
            .iter()
            .take(200)
            .map(|prediction| OutcomeView {
                made: time_label(prediction.made_ms, intraday),
                brain: prediction.source.clone(),
                step: prediction.step as u32,
                due: time_label(prediction.due_ms, intraday),
                from: prediction.from,
                forecast: prediction.close,
                actual: prediction.outcome.map(|outcome| outcome.actual),
                error_pct: prediction.outcome.map(|outcome| outcome.error_pct),
                inside: prediction.outcome.and_then(|outcome| outcome.inside),
                result: match prediction.outcome {
                    None => "waiting",
                    Some(outcome) => match outcome.right {
                        Some(true) => "right",
                        Some(false) => "wrong",
                        None => "flat",
                    },
                }
                .to_string(),
            })
            .collect();

        OutcomesView {
            live: snapshot.source.as_ref().is_some_and(Source::is_live),
            tallies,
            outcomes,
        }
    }

    pub fn metrics(&self) -> MetricsView {
        let snapshot = self.market.snapshot();
        let llm = &snapshot.llm;

        let all = self.market.all_tallies();
        let (llm_right, llm_judged) = all
            .iter()
            .filter(|tally| tally.source == "LLM")
            .fold((0, 0), |(right, judged), tally| {
                (right + tally.right as u32, judged + tally.judged as u32)
            });

        let step_view = |score: &cortex_liquid::StepScore| StepView {
            step: score.step as u32,
            accuracy: score.accuracy,
            low: score.low,
            high: score.high,
        };

        let (nets, baseline) = match &snapshot.report {
            Some(report) => (
                report
                    .metrics
                    .iter()
                    .map(|metrics| {
                        let brain = Brain::of(metrics.net);
                        let mut facts: Vec<FactView> = metrics
                            .facts
                            .iter()
                            .map(|(name, value)| FactView {
                                name: name.clone(),
                                value: value.clone(),
                            })
                            .collect();

                        if metrics.net == cortex_liquid::Net::Spiking && snapshot.spikes > 0 {
                            facts.push(FactView {
                                name: "Spikes, last candle".to_string(),
                                value: snapshot.spikes.to_string(),
                            });
                        }

                        NetMetricsView {
                            brain: brain.short().to_string(),
                            name: brain.name().to_string(),
                            about: brain.about().to_string(),
                            selected: brain == snapshot.brain,
                            steps: metrics.steps.iter().map(step_view).collect(),
                            error_pct: metrics.at(1).map(|score| score.error_pct),
                            skill: metrics.at(1).map(|score| score.skill),
                            band_hit: metrics.band_hit,
                            volume_error: metrics.volume_error,
                            parameters: metrics.parameters as u64,
                            neurons: metrics.neurons as u64,
                            weight: metrics.weight,
                            train_ms: metrics.train_ms,
                            facts,
                            loss_fit: metrics.losses.iter().map(|loss| loss.fit).collect(),
                            loss_check: metrics.losses.iter().map(|loss| loss.check).collect(),
                        }
                    })
                    .collect(),
                report.baseline.iter().map(step_view).collect(),
            ),
            None => (Vec::new(), Vec::new()),
        };

        MetricsView {
            trained: snapshot.report.is_some(),
            trained_on: snapshot.trained_on.clone(),
            bars: snapshot
                .report
                .as_ref()
                .map_or(0, |report| report.bars as u32),
            held_out: snapshot
                .report
                .as_ref()
                .map_or(0, |report| (report.bars - report.train_bars) as u32),
            total_ms: snapshot
                .report
                .as_ref()
                .map_or(0.0, |report| report.total_ms),
            nets,
            baseline,
            llm_model: llm.model.clone(),
            llm_calls: llm.calls,
            llm_failures: llm.failures,
            llm_mean_ms: if llm.calls > 0 {
                llm.total_ms / f64::from(llm.calls)
            } else {
                0.0
            },
            llm_live_right: llm_right,
            llm_live_judged: llm_judged,
        }
    }
}

/// A price as the screens write it: grouped, with the decimals its size
/// needs.
#[uniffi::export]
pub fn price_text(price: f64) -> String {
    cortex_market::price_text(price)
}

/// `14:35` or `09 Mar`, in India.
#[uniffi::export]
pub fn market_time(time_ms: i64, intraday: bool) -> String {
    if intraday {
        clock::moment_text(time_ms)
    } else {
        clock::day_text(time_ms)
    }
}
