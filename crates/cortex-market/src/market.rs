//! One instrument being watched: its bars, live from Dhan or loaded from
//! a file or a picture, the networks trained on them, their forecasts,
//! the plan, and the record of how earlier forecasts came out.
//!
//! The work happens on background threads; a screen calls
//! [`Market::snapshot`] whenever it is told something changed, and draws
//! what it gets.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use cortex_liquid::{Account, Bar, Forecaster, Net, PathReport, PathSettings, PredictedCandle};
use serde::{Deserialize, Serialize};
use tokio::runtime::Runtime;
use tokio::sync::watch;

use crate::clock::{self, Session};
use crate::dhan::depth::{self, DepthMode};
use crate::dhan::feed::{self, FeedNews, Packet, Tick};
use crate::dhan::instruments::{self, Instrument};
use crate::dhan::{Credentials, Dhan, Level, Profile, Quote};
use crate::flow::DepthSnapshot;
use crate::indicators::{self, Indicators, Levels};
use crate::kinds::{Brain, Interval, Source};
use crate::llm::{self, Oracle, Picture};
use crate::outcomes::{Ledger, Prediction, RECORDED_STEPS, Tally};
use crate::picture;
use crate::plan::{self, Plan, Reading};

/// Closed bars after which the networks are trained again.
const RETRAIN_AFTER: usize = 60;

/// The most bars, the latest, trained on: enough to learn from, few
/// enough to train in a second or two on a phone.
const MOST_TRAINED: usize = 3_000;

/// The least time between two notices of a tick to the screen.
const TICK_NOTICE: Duration = Duration::from_millis(250);

/// The connection to Dhan.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Connection {
    #[default]
    Off,
    Checking,
    Connected(Profile),
    Failed(String),
}

/// The live feed.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum FeedState {
    #[default]
    Off,
    Connecting,
    Live,

    /// Lost for a moment; trying again.
    Retrying(String),
    Stopped(String),
}

/// What the background work is doing.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Work {
    #[default]
    Idle,
    Loading(String),
    Training(f32),
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum Asking {
    #[default]
    Idle,
    Asking,
    Failed(String),
}

/// The language model's latest forecast and how it has been asked.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LlmState {
    /// The model's name, when one is set.
    pub model: Option<String>,
    pub asking: Asking,
    pub candles: Vec<PredictedCandle>,
    pub bias: String,
    pub reason: String,

    /// The bar its latest forecast was made at.
    pub made_ms: i64,
    pub calls: u32,
    pub failures: u32,
    pub total_ms: f64,
}

/// The numbers for right now.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct Now {
    pub price: f64,
    pub previous_close: Option<f64>,
    pub change: Option<f64>,
    pub change_pct: Option<f64>,
    pub vwap: Option<f64>,
    pub rsi: Option<f64>,
    pub ema_fast: Option<f64>,
    pub ema_slow: Option<f64>,
    pub atr: Option<f64>,

    /// Traded today.
    pub volume: Option<f64>,
    pub day_high: Option<f64>,
    pub day_low: Option<f64>,

    /// The order book: best bid and ask, the gap, and the quantities.
    pub bid: Option<f64>,
    pub ask: Option<f64>,
    pub spread: Option<f64>,
    pub bid_quantity: Option<f64>,
    pub ask_quantity: Option<f64>,

    /// Of the waiting quantity, the share that is buying, from 0 to 1.
    pub buy_share: Option<f64>,
    pub open_interest: Option<f64>,

    /// When the last tick came.
    pub tick_ms: Option<i64>,
}

/// One row of the forecast table.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Row {
    pub step: usize,
    pub time_ms: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,

    /// The 80% band of the close.
    pub close_low: f64,
    pub close_high: f64,

    /// Change from the latest close, in percent.
    pub change_pct: f64,
    pub rsi: Option<f64>,
    pub vwap: Option<f64>,

    /// Price times volume.
    pub turnover: f64,
}

/// Everything a screen shows, at one moment.
#[derive(Clone, Default)]
pub struct Snapshot {
    pub revision: u64,

    pub connection: Connection,
    pub source: Option<Source>,
    pub interval: Interval,
    pub brain: Brain,

    pub feed: FeedState,
    pub work: Work,
    pub problem: Option<String>,
    pub notes: Vec<String>,

    /// Closed bars, oldest first, and the one still forming.
    pub bars: Arc<Vec<Bar>>,
    pub forming: Option<Bar>,
    pub indicators: Arc<Indicators>,

    pub now: Now,
    pub levels: Levels,

    /// The chosen brain's next ten candles.
    pub rows: Vec<Row>,
    pub plan: Option<Plan>,

    /// The networks' measured record on held-out bars.
    pub report: Option<Arc<PathReport>>,

    /// What the networks were trained on, and the bars they have seen
    /// since.
    pub trained_on: String,
    pub since_training: usize,

    pub llm: LlmState,
    pub account: Account,
    pub spikes: u32,

    /// Recorded forecasts of this instrument, newest first.
    pub outcomes: Vec<Prediction>,
    pub tallies: Vec<Tally>,
    pub depth: DepthSnapshot,
    pub networks: Vec<(String, Vec<PredictedCandle>)>,
    pub flow_history: Vec<crate::flow::FlowSample>,
}

/// What is kept between runs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Kept {
    instrument: Option<Instrument>,
    interval: Option<Interval>,
    brain: Option<Brain>,
    #[serde(default)]
    depth_mode: Option<DepthMode>,
}

#[cfg(feature = "desktop")]
#[derive(Serialize, Deserialize)]
struct SavedModel {
    schema: u32,
    scope: String,
    interval: Interval,
    forecaster: Forecaster,
    report: PathReport,
}

struct State {
    snapshot: Snapshot,
    credentials: Option<Credentials>,
    instruments: Vec<Instrument>,

    forecaster: Option<Forecaster>,
    training: bool,

    /// Counts sources, so that work for one since replaced is dropped.
    generation: u64,
    stop_feed: Option<watch::Sender<bool>>,

    /// The day's traded volume when the forming candle began.
    volume_at_start: Option<f64>,
    picture: Option<Picture>,
    picture_prices: Option<(f64, f64)>,
    ledger: Ledger,
    oracle: Option<Arc<dyn Oracle>>,
    last_notice: Instant,
    depth_mode: DepthMode,
    depth_configured: bool,
    depth_generation: u64,
    stop_depth: Option<watch::Sender<bool>>,
    #[cfg(feature = "desktop")]
    learning: Option<Arc<crate::learning::Learning>>,
    #[cfg(feature = "desktop")]
    last_observed: i64,
}

struct Inner {
    folder: PathBuf,
    runtime: &'static Runtime,
    state: Mutex<State>,
    changed: Mutex<Vec<(u64, Box<dyn Fn() + Send + Sync>)>>,
    #[cfg(feature = "desktop")]
    desktop_root: Mutex<Option<PathBuf>>,
}

/// One instrument being watched. Cheap to clone; every clone is the same.
#[derive(Clone)]
pub struct Market {
    inner: Arc<Inner>,
}

impl Market {
    /// Keeps its files in `folder`. `changed` is called, from any thread,
    /// whenever there is something new to show. Reconnects to Dhan and
    /// watches the instrument of the last run, if there was one.
    pub fn new(folder: &Path, changed: impl Fn() + Send + Sync + 'static) -> Self {
        static RUNTIME: OnceLock<Runtime> = OnceLock::new();
        let runtime = RUNTIME.get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("market")
                .enable_all()
                .build()
                .expect("the market needs its background threads")
        });

        let _ = std::fs::create_dir_all(folder);

        let kept: Kept = read_json(&folder.join("market.json")).unwrap_or_default();
        let credentials: Option<Credentials> = read_json(&folder.join("dhan.json"));
        let instruments: Vec<Instrument> =
            read_json(&folder.join("instruments.json")).unwrap_or_default();

        let snapshot = Snapshot {
            interval: kept.interval.unwrap_or_default(),
            brain: kept.brain.unwrap_or_default(),
            ..Snapshot::default()
        };

        let market = Market {
            inner: Arc::new(Inner {
                folder: folder.to_path_buf(),
                runtime,
                state: Mutex::new(State {
                    snapshot,
                    credentials: credentials.clone(),
                    instruments,
                    forecaster: None,
                    training: false,
                    generation: 0,
                    stop_feed: None,
                    volume_at_start: None,
                    picture: None,
                    picture_prices: None,
                    ledger: Ledger::open(&folder.join("outcomes.json")),
                    oracle: None,
                    last_notice: Instant::now(),
                    depth_mode: kept.depth_mode.unwrap_or_default(),
                    depth_configured: kept.depth_mode.is_some(),
                    depth_generation: 0,
                    stop_depth: None,
                    #[cfg(feature = "desktop")]
                    learning: None,
                    #[cfg(feature = "desktop")]
                    last_observed: 0,
                }),
                changed: Mutex::new(vec![(0, Box::new(changed))]),
                #[cfg(feature = "desktop")]
                desktop_root: Mutex::new(None),
            }),
        };

        if let Some(credentials) = credentials {
            // An address of its own is the stand-in's, which trades
            // around the clock.
            clock::set_always_open(!credentials.api.is_empty());
            market.check(credentials, kept.instrument);
        }

        market
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn tell(&self) {
        for (_, changed) in self.inner.changed.lock().unwrap().iter() {
            changed();
        }
    }

    #[cfg(feature = "desktop")]
    pub(crate) fn listen(&self, changed: impl Fn() + Send + Sync + 'static) -> u64 {
        static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.inner
            .changed
            .lock()
            .unwrap()
            .push((id, Box::new(changed)));
        id
    }
    #[cfg(feature = "desktop")]
    pub(crate) fn unlisten(&self, id: u64) {
        self.inner
            .changed
            .lock()
            .unwrap()
            .retain(|(key, _)| *key != id);
    }
    #[cfg(feature = "desktop")]
    pub(crate) fn set_desktop_root(&self, folder: &Path) {
        *self.inner.desktop_root.lock().unwrap() = Some(folder.into());
    }
    #[cfg(feature = "desktop")]
    pub(crate) fn pause_subscription(&self) {
        let mut state = self.state();
        if let Some(stop) = state.stop_feed.take() {
            let _ = stop.send(true);
        }
        if let Some(stop) = state.stop_depth.take() {
            let _ = stop.send(true);
        }
        state.generation += 1;
        state.depth_generation += 1;
        state.snapshot.feed = FeedState::Off;
        state.snapshot.depth = DepthSnapshot::default();
        bump(&mut state);
    }

    /// Changes whenever anything in the snapshot does.
    pub fn revision(&self) -> u64 {
        self.state().snapshot.revision
    }

    pub fn snapshot(&self) -> Snapshot {
        self.state().snapshot.clone()
    }

    pub fn depth_mode(&self) -> DepthMode {
        self.state().depth_mode
    }

    pub fn set_depth_mode(&self, mode: DepthMode) {
        {
            let mut state = self.state();
            state.depth_mode = mode;
            state.depth_configured = true;
        }
        self.keep();
        self.start_depth();
        self.tell();
    }

    fn start_depth(&self) {
        if !cfg!(feature = "desktop") {
            return;
        }
        #[cfg(feature = "desktop")]
        if self.state().learning.is_none() {
            return;
        }
        let (credentials, instrument, mode, generation, depth_generation, stopped) = {
            let mut state = self.state();
            if let Some(stop) = state.stop_depth.take() {
                let _ = stop.send(true);
            }
            state.depth_generation += 1;
            state.snapshot.depth = DepthSnapshot::default();
            if state.depth_mode == DepthMode::Five {
                return;
            }
            let Some(Source::Dhan { instrument }) = state.snapshot.source.clone() else {
                return;
            };
            if !matches!(
                instrument.segment,
                crate::Segment::NseEquity | crate::Segment::NseDerivatives
            ) {
                state.snapshot.depth.status =
                    "Deep depth unavailable for this segment; Full feed fallback".into();
                return;
            }
            let Some(credentials) = state.credentials.clone() else {
                return;
            };
            if !credentials.api.is_empty() && std::env::var("CORTEX_DHAN_DEPTH").is_err() {
                return;
            }
            let (tx, rx) = watch::channel(false);
            state.stop_depth = Some(tx);
            state.snapshot.depth.status = "Connecting depth…".into();
            (
                credentials,
                instrument,
                state.depth_mode,
                state.generation,
                state.depth_generation,
                rx,
            )
        };
        let market = self.clone();
        let id = instrument.security_id.parse::<u32>().unwrap_or_default();
        let segment = instrument.segment.number();
        self.inner.runtime.spawn(depth::watch(credentials,instrument,mode,stopped,move |news| {
            let mut state=market.state();if state.generation!=generation || state.depth_generation!=depth_generation {return;}
            match news {
                depth::News::Connecting=>state.snapshot.depth.status="Connecting depth…".into(),
                depth::News::Packet(packet) if packet.security_id==id && packet.segment==segment=>{
                    #[cfg(feature="desktop")]
                    if let Some(learning)=&state.learning {learning.recorder.record(serde_json::json!({"received_ms":clock::now_ms(),"kind":"depth","source":state.snapshot.source,"bid":packet.bid,"levels":packet.levels,"coverage":mode.levels()}));}

                    if state.snapshot.depth.coverage!=mode.levels() {state.snapshot.depth=DepthSnapshot::default();}
                    state.snapshot.depth.set_side(packet.bid,packet.levels,clock::now_ms(),mode.levels());
                    state.snapshot.depth.status=format!("Dhan · {} levels",mode.levels());
                },
                depth::News::Packet(_)=>{},
                depth::News::Dropped=>{state.snapshot.depth=DepthSnapshot::default();state.snapshot.depth.status="Depth reconnecting; Full feed fallback".into();},
                depth::News::Stopped(reason)=>{state.snapshot.depth=DepthSnapshot::default();state.snapshot.depth.status=reason;},
            }
            refresh_flow(&mut state.snapshot,clock::now_ms());
                bump(&mut state);drop(state);market.notice();
            #[cfg(feature="desktop")]
            market.observe_desktop();
        }));
    }

    #[cfg(feature = "desktop")]
    pub fn enable_learning(&self, learning: Arc<crate::learning::Learning>) {
        let mut state = self.state();
        if state
            .learning
            .as_ref()
            .is_some_and(|existing| Arc::ptr_eq(existing, &learning))
        {
            return;
        }
        if !state.depth_configured {
            state.depth_mode = DepthMode::Twenty;
        }
        state.learning = Some(learning);
        drop(state);
        self.start_depth();
    }

    #[cfg(feature = "desktop")]
    fn observe_desktop(&self) {
        use cortex_liquid::abilities::{INPUTS, Perception};
        let mut state = self.state();
        let now = clock::now_ms();
        if now - state.last_observed < 10_000
            || state.snapshot.now.price <= 0.
            || !state.snapshot.now.tick_ms.is_some_and(|t| now - t <= 5000)
        {
            return;
        }
        let Some(learning) = state.learning.clone() else {
            return;
        };
        let Some(source) = state.snapshot.source.as_ref() else {
            return;
        };
        let scope = format!("{}:{}", source.key(), state.snapshot.interval.label());
        let price = state.snapshot.now.price;
        let flow = state.snapshot.depth.features(now);
        let mut values = [0.; INPUTS];
        let previous = state.snapshot.bars.last().map_or(price, |b| b.close);
        values[0] = ((price / previous).ln() * 100.).clamp(-5., 5.) as f32;
        values[1] = flow.imbalance.unwrap_or_default() as f32;
        values[2] =
            (state.snapshot.now.atr.unwrap_or_default() / price * 100.).clamp(0., 5.) as f32;
        values[3] = if flow.available { 1. } else { 0. };
        values[4] = (flow.spread.unwrap_or_default() / price * 100.).clamp(0., 5.) as f32;
        values[5] = (state.snapshot.now.rsi.unwrap_or(50.) / 50. - 1.) as f32;
        values[6] = (state.snapshot.now.buy_share.unwrap_or(0.5) * 2. - 1.) as f32;
        values[7] = if matches!(state.snapshot.feed, FeedState::Live) {
            1.
        } else {
            0.
        };
        values[8] = (flow.microprice.unwrap_or(price) / price - 1.).clamp(-0.1, 0.1) as f32 * 100.;
        if let Some(flow) = state.snapshot.flow_history.last() {
            values[9] = (flow.volume_delta.unwrap_or_default().ln_1p() / 20.).clamp(0., 1.) as f32;
            values[10] = (flow.displayed_change.unwrap_or_default().signum()
                * (flow.displayed_change.unwrap_or_default().abs().ln_1p() / 20.))
                .clamp(-1., 1.) as f32;
            values[11] = if flow.volume_delta.is_some() { 1. } else { 0. };
        }

        let percepts: Vec<String> = vec![
            "domain: trading".into(),
            format!("instrument: {}", source.key()),
            format!(
                "movement: {}",
                if values[0] > 0.01 {
                    "up"
                } else if values[0] < -0.01 {
                    "down"
                } else {
                    "flat"
                }
            ),
            format!(
                "liquidity: {}",
                if !flow.available {
                    "unknown"
                } else if values[1] > 0.2 {
                    "bid heavy"
                } else if values[1] < -0.2 {
                    "ask heavy"
                } else {
                    "balanced"
                }
            ),
        ];
        // What each forecaster expects of the next candle goes with the
        // bar, so that the brain learns whose calls come true when.
        let mut percepts = percepts;
        for (name, candles) in &state.snapshot.networks {
            if let Some(next) = candles.first() {
                let change = (next.close / price - 1.) * 100.;
                let name: String = name
                    .chars()
                    .filter(|c| c.is_alphanumeric())
                    .take(16)
                    .collect::<String>()
                    .to_lowercase();
                percepts.push(format!(
                    "{name}: expects {}",
                    if change > 0.01 {
                        "up"
                    } else if change < -0.01 {
                        "down"
                    } else {
                        "flat"
                    }
                ));
            }
        }
        let evidence = serde_json::json!({"revision":state.snapshot.revision,"price":price,"depth":state.snapshot.depth,"flow":flow,"feature_version":1});
        state.last_observed = now;
        drop(state);
        learning.observe(
            Perception {
                domain: "trading".into(),
                scope,
                time_ms: now,
                values,
                percepts,
                expected: Vec::new(),
            },
            evidence,
        );
    }

    pub fn context(&self) -> serde_json::Value {
        let snapshot = self.snapshot();
        let now = clock::now_ms();
        let scope = snapshot
            .source
            .as_ref()
            .map(Source::key)
            .unwrap_or_else(|| "*".into());
        let bars = &snapshot.bars[snapshot.bars.len().saturating_sub(80)..];
        let mut context = serde_json::json!({"revision":snapshot.revision,"as_of_ms":now,"scope":scope,
            "source":snapshot.source,"interval":snapshot.interval.label(),"feed":format!("{:?}",snapshot.feed),
            "tick_age_ms":snapshot.now.tick_ms.map(|t|now-t),"now":snapshot.now,"bars":bars,"forming":snapshot.forming,
            "levels":snapshot.levels,"depth":snapshot.depth,"flow":snapshot.depth.features(now),"networks":snapshot.networks,
            "flow_history":snapshot.flow_history,"rows":snapshot.rows,"plan":snapshot.plan,"metrics":snapshot.report.as_deref(),
            "outcomes":snapshot.outcomes.iter().take(20).collect::<Vec<_>>(),"tallies":snapshot.tallies,
            "trained_on":snapshot.trained_on,"feature_version":1,"notes":snapshot.notes});
        #[cfg(feature = "desktop")]
        if let Some(learning) = self.state().learning.clone() {
            context["adaptive"] = serde_json::to_value(learning.reading(
                cortex_brain::memory::experience::Domain::Trading,
                &format!("{scope}:{}", snapshot.interval.label()),
            ))
            .unwrap_or_default();
            context["learning_problem"] = serde_json::json!(learning.problem());
            context["recording"] = serde_json::json!(learning.recorder.counts());
        }
        context
    }

    #[cfg(feature = "desktop")]
    fn model_file(&self, scope: &str, interval: Interval) -> PathBuf {
        let hash = scope.bytes().fold(14695981039346656037u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(1099511628211)
        });
        self.inner
            .folder
            .join("models")
            .join(format!("{hash:x}-{}.json", interval.label()))
    }
    #[cfg(feature = "desktop")]
    fn restore_model(&self, generation: u64) {
        let (scope, interval) = {
            let state = self.state();
            if state.learning.is_none() {
                return;
            }
            let Some(source) = &state.snapshot.source else {
                return;
            };
            (source.key(), state.snapshot.interval)
        };
        let file = self.model_file(&scope, interval);
        let saved = read_json::<SavedModel>(&file)
            .or_else(|| read_json::<SavedModel>(&file.with_extension("previous.json")));
        let Some(saved) = saved.filter(|s| {
            s.schema == 1 && s.scope == scope && s.interval == interval && s.forecaster.is_sound()
        }) else {
            return;
        };
        let mut state = self.state();
        if state.generation != generation {
            return;
        }
        state.forecaster = Some(saved.forecaster);
        state.snapshot.report = Some(Arc::new(saved.report));
        state.snapshot.trained_on = format!("{scope} {} · restored", interval.label());
    }
    #[cfg(feature = "desktop")]
    fn save_model(&self, generation: u64) {
        let saved = {
            let state = self.state();
            if state.learning.is_none() || state.generation != generation {
                return;
            }
            let (Some(source), Some(forecaster), Some(report)) = (
                &state.snapshot.source,
                &state.forecaster,
                &state.snapshot.report,
            ) else {
                return;
            };
            SavedModel {
                schema: 1,
                scope: source.key(),
                interval: state.snapshot.interval,
                forecaster: forecaster.clone(),
                report: report.as_ref().clone(),
            }
        };
        let file = self.model_file(&saved.scope, saved.interval);
        if let Err(error) = crate::learning::checkpoint_write(&file, &saved) {
            let mut state = self.state();
            state.snapshot.problem = Some(format!("Model checkpoint failed: {error}"));
            bump(&mut state);
        }
    }
    #[cfg(feature = "desktop")]
    fn remember_outcomes(&self) {
        use cortex_brain::memory::experience::{Domain, Experience};
        let (learning, snapshot) = {
            let state = self.state();
            let Some(learning) = state.learning.clone() else {
                return;
            };
            (learning, state.snapshot.clone())
        };
        for prediction in snapshot.outcomes.iter().take(40) {
            let resolved = prediction.outcome.is_some();
            let now = clock::now_ms();
            let event = Experience {
                id: 0,
                key: format!(
                    "{}:{}:{}:{}:{}",
                    if resolved { "resolved" } else { "prediction" },
                    prediction.instrument,
                    prediction.interval,
                    prediction.made_ms,
                    format_args!("{}:{}", prediction.source, prediction.step)
                ),
                domain: Domain::Trading,
                scope: prediction.instrument.clone(),
                session: String::new(),
                kind: if resolved { "outcome" } else { "prediction" }.into(),
                created_ms: prediction.made_ms,
                available_ms: now,
                text: format!(
                    "{} {} {} step {} forecast {:.2}, realized {:?}",
                    prediction.source,
                    prediction.symbol,
                    prediction.interval,
                    prediction.step,
                    prediction.close,
                    prediction.outcome
                ),
                evidence: serde_json::json!(prediction),
                verified: resolved,
            };
            if let Err(error) = learning.memory.append(&event) {
                let mut state = self.state();
                state.snapshot.problem = Some(error.to_string());
                bump(&mut state);
            }
        }
    }

    fn keep(&self) {
        let state = self.state();

        let kept = Kept {
            instrument: match &state.snapshot.source {
                Some(Source::Dhan { instrument }) => Some(instrument.clone()),
                _ => None,
            },
            interval: Some(state.snapshot.interval),
            brain: Some(state.snapshot.brain),
            depth_mode: Some(state.depth_mode),
        };

        drop(state);

        write_json(&self.inner.folder.join("market.json"), &kept, false);
        #[cfg(feature = "desktop")]
        if let Some(root) = self.inner.desktop_root.lock().unwrap().as_ref() {
            write_json(&root.join("market.json"), &kept, false);
        }
    }

    // Dhan ---------------------------------------------------------------

    /// Connects with a client ID and access token, and keeps them for
    /// next time if they work.
    pub fn connect(&self, client_id: &str, access_token: &str) {
        let credentials = Credentials::new(client_id, access_token);

        if credentials.client_id.is_empty() || credentials.access_token.is_empty() {
            let mut state = self.state();

            state.snapshot.connection =
                Connection::Failed("Enter both the client ID and the access token.".to_string());
            bump(&mut state);
            drop(state);
            self.tell();

            return;
        }

        let watching = match &self.state().snapshot.source {
            Some(Source::Dhan { instrument }) => Some(instrument.clone()),
            _ => None,
        };

        self.check(credentials, watching);
    }

    fn check(&self, credentials: Credentials, then_watch: Option<Instrument>) {
        {
            let mut state = self.state();

            state.snapshot.connection = Connection::Checking;
            bump(&mut state);
        }

        self.tell();

        let market = self.clone();

        self.inner.runtime.spawn(async move {
            let checked = Dhan::new(credentials.clone()).profile().await;

            {
                let mut state = market.state();

                match &checked {
                    Ok(profile) => {
                        state.snapshot.connection = Connection::Connected(profile.clone());
                        state.credentials = Some(credentials.clone());
                    }
                    Err(error) => {
                        state.snapshot.connection = Connection::Failed(error.to_string());
                    }
                }

                bump(&mut state);
            }

            if checked.is_ok() {
                write_json(&market.inner.folder.join("dhan.json"), &credentials, true);
                #[cfg(feature = "desktop")]
                if let Some(root) = market.inner.desktop_root.lock().unwrap().as_ref() {
                    write_json(&root.join("dhan.json"), &credentials, true);
                }

                market.tell();

                let instrument = then_watch.unwrap_or_else(|| instruments::favourites()[0].clone());

                market.watch(instrument);
            } else {
                market.tell();
            }
        });
    }

    /// Forgets the token and stops the feed.
    pub fn disconnect(&self) {
        {
            let mut state = self.state();

            if let Some(stop) = state.stop_feed.take() {
                let _ = stop.send(true);
            }

            state.credentials = None;
            state.snapshot.connection = Connection::Off;
            state.snapshot.feed = FeedState::Off;
            bump(&mut state);
        }

        let _ = std::fs::remove_file(self.inner.folder.join("dhan.json"));

        self.tell();
    }

    pub fn is_connected(&self) -> bool {
        matches!(self.state().snapshot.connection, Connection::Connected(_))
    }

    /// The saved client ID, to fill the form with.
    pub fn client_id(&self) -> Option<String> {
        self.state()
            .credentials
            .as_ref()
            .map(|credentials| credentials.client_id.clone())
    }

    /// Instruments matching `query`: the favourites, and Dhan's whole
    /// list once it has been fetched.
    pub fn search(&self, query: &str, most: usize) -> Vec<Instrument> {
        let state = self.state();
        let favourites = instruments::favourites();

        let mut found = instruments::search(&favourites, query, most);

        if !state.instruments.is_empty() {
            for instrument in instruments::search(&state.instruments, query, most) {
                if found.len() >= most {
                    break;
                }

                if !found.iter().any(|known| known.key() == instrument.key()) {
                    found.push(instrument);
                }
            }
        }

        found
    }

    /// Whether Dhan's whole list of instruments is here.
    pub fn has_instrument_list(&self) -> bool {
        !self.state().instruments.is_empty()
    }

    /// Fetches Dhan's list of instruments, to search beyond the
    /// favourites. It is large (tens of megabytes) and fetched once.
    pub fn fetch_instrument_list(&self) {
        let market = self.clone();

        {
            let mut state = self.state();

            state.snapshot.work = Work::Loading("the list of instruments".to_string());
            bump(&mut state);
        }

        self.tell();

        self.inner.runtime.spawn(async move {
            let fetched = async {
                let text = reqwest::get(instruments::SCRIP_MASTER)
                    .await
                    .map_err(|error| error.to_string())?
                    .text()
                    .await
                    .map_err(|error| error.to_string())?;

                Ok::<_, String>(instruments::read_scrip_master(&text))
            }
            .await;

            {
                let mut state = market.state();

                match fetched {
                    Ok(list) if !list.is_empty() => {
                        write_json(&market.inner.folder.join("instruments.json"), &list, false);
                        state.instruments = list;
                        state.snapshot.work = Work::Idle;
                    }
                    Ok(_) => {
                        state.snapshot.work =
                            Work::Failed("Dhan's list of instruments was empty.".to_string());
                    }
                    Err(error) => {
                        state.snapshot.work =
                            Work::Failed(format!("Cannot fetch the instrument list: {error}"));
                    }
                }

                bump(&mut state);
            }

            market.tell();
        });
    }

    // Choices --------------------------------------------------------------

    pub fn set_brain(&self, brain: Brain) {
        {
            let mut state = self.state();

            if state.snapshot.brain == brain {
                return;
            }

            state.snapshot.brain = brain;
            derive(&mut state);
        }

        self.keep();
        self.tell();

        if brain == Brain::Llm {
            self.ask_llm(false);
        }
    }

    /// The candle size. Fetches the bars again for a live instrument.
    pub fn set_interval(&self, interval: Interval) {
        let instrument = {
            let mut state = self.state();

            if state.snapshot.interval == interval {
                return;
            }

            state.snapshot.interval = interval;
            bump(&mut state);

            match &state.snapshot.source {
                Some(Source::Dhan { instrument }) => Some(instrument.clone()),
                _ => None,
            }
        };

        self.keep();

        match instrument {
            Some(instrument) => self.watch(instrument),
            None => self.tell(),
        }
    }

    /// The model the language-model forecast asks.
    pub fn set_oracle(&self, oracle: Option<Arc<dyn Oracle>>) {
        let mut state = self.state();

        state.snapshot.llm.model = oracle.as_ref().map(|oracle| oracle.name());
        state.oracle = oracle;
        bump(&mut state);
    }

    // Sources --------------------------------------------------------------

    /// Starts afresh on a new source: stops the feed, drops the bars.
    fn begin(&self, source: Source) -> u64 {
        let mut state = self.state();

        if let Some(stop) = state.stop_feed.take() {
            let _ = stop.send(true);
        }
        if let Some(stop) = state.stop_depth.take() {
            let _ = stop.send(true);
        }
        state.depth_generation += 1;
        state.snapshot.depth = DepthSnapshot::default();
        state.snapshot.flow_history.clear();

        state.generation += 1;
        state.snapshot.source = Some(source);
        state.snapshot.flow_history.clear();
        state.snapshot.bars = Arc::new(Vec::new());
        state.snapshot.forming = None;
        state.snapshot.feed = FeedState::Off;
        state.snapshot.problem = None;
        state.snapshot.notes.clear();
        state.snapshot.now = Now::default();
        state.snapshot.llm.candles.clear();
        state.snapshot.llm.made_ms = 0;
        state.snapshot.llm.reason.clear();
        state.snapshot.llm.asking = Asking::Idle;
        state.volume_at_start = None;
        state.picture = None;
        state.picture_prices = None;

        derive(&mut state);

        state.generation
    }

    /// Watches an instrument on Dhan: fetches its history, trains on it,
    /// and follows the live feed.
    pub fn watch(&self, instrument: Instrument) {
        let Some(credentials) = self.state().credentials.clone() else {
            let mut state = self.state();

            state.snapshot.problem = Some("Connect Dhan first.".to_string());
            bump(&mut state);
            drop(state);
            self.tell();

            return;
        };

        let generation = self.begin(Source::Dhan {
            instrument: instrument.clone(),
        });

        let interval = self.state().snapshot.interval;

        {
            let mut state = self.state();

            state.snapshot.work = Work::Loading(format!("{} history", instrument.symbol));
            bump(&mut state);
        }

        self.keep();
        self.tell();

        let market = self.clone();

        self.inner.runtime.spawn(async move {
            #[cfg(feature = "desktop")]
            market.restore_model(generation);
            let dhan = Dhan::new(credentials.clone());
            let now = clock::now_ms();
            let from = now - interval.history_days() * clock::DAY_MS;

            let history = match interval.minutes() {
                Some(minutes) => dhan.intraday(&instrument, minutes, from, now).await,
                None => dhan.daily(&instrument, from, now).await,
            };

            let quote = dhan.quote(&instrument).await.ok();

            let bars = match history {
                Ok(bars) => bars,
                Err(error) => {
                    let mut state = market.state();

                    if state.generation == generation {
                        state.snapshot.work = Work::Failed(error.to_string());
                        bump(&mut state);
                    }

                    drop(state);
                    market.tell();

                    return;
                }
            };

            {
                let mut state = market.state();

                if state.generation != generation {
                    return;
                }

                let session = instrument.segment.session();
                let (closed, forming) = split_forming(bars, interval, session, now);

                state.snapshot.bars = Arc::new(closed);
                state.snapshot.forming = forming;

                if let Some(quote) = quote {
                    apply_quote(&mut state.snapshot.now, &quote);
                    state.volume_at_start =
                        Some(quote.volume - forming.map_or(0.0, |bar| bar.volume));
                }

                state.snapshot.work = Work::Idle;
                derive(&mut state);
            }

            market.tell();
            market.train(generation);
            market.follow(generation, credentials, instrument, interval);
        });
    }

    /// Fetches the history again.
    pub fn refresh(&self) {
        let source = self.state().snapshot.source.clone();

        if let Some(Source::Dhan { instrument }) = source {
            self.watch(instrument);
        }
    }

    /// Reads a CSV file of bars.
    pub fn load_csv(&self, name: &str, text: &str) -> Result<usize, String> {
        let parsed = cortex_liquid::parse_csv(text).map_err(|error| format!("{name}: {error}"))?;
        let count = parsed.bars.len();

        let generation = self.begin(Source::File {
            name: name.to_string(),
        });

        {
            let mut state = self.state();

            let gap = median_gap(&parsed.bars);

            state.snapshot.interval = Interval::nearest(gap);
            state.snapshot.notes = parsed.notes;
            state.snapshot.bars = Arc::new(parsed.bars);
            derive(&mut state);
        }

        self.tell();
        self.train(generation);

        Ok(count)
    }

    /// Reads candles off a picture of a chart. `prices` are the highest
    /// and lowest prices on it, if known; a language model, if one is
    /// set, is asked to read them off the axis otherwise.
    pub fn load_picture(
        &self,
        name: &str,
        bytes: Vec<u8>,
        prices: Option<(f64, f64)>,
    ) -> Result<usize, String> {
        let interval = self.state().snapshot.interval;
        let now = clock::now_ms();
        let read = picture::read_chart(&bytes, prices, interval.ms(), now)?;
        let count = read.bars.len();

        let generation = self.begin(Source::Picture {
            name: name.to_string(),
        });

        {
            let mut state = self.state();

            state.snapshot.notes = read.notes;
            state.snapshot.bars = Arc::new(read.bars);
            state.picture = Some(Picture {
                media_type: media_type(&bytes).to_string(),
                bytes,
            });
            state.picture_prices = prices;
            derive(&mut state);
        }

        self.tell();
        self.train(generation);

        // The model can read what the networks cannot: the axis, the
        // pattern as a whole.
        if self.state().oracle.is_some() {
            self.ask_llm(true);
        }

        Ok(count)
    }

    // Training -------------------------------------------------------------

    /// Trains the networks on the bars in the background, or, where there
    /// are too few, runs the last trained networks on them.
    fn train(&self, generation: u64) {
        let (bars, settings) = {
            let mut state = self.state();

            if state.training || state.generation != generation {
                return;
            }

            let all = state.snapshot.bars.clone();
            let bars = Arc::new(all[all.len().saturating_sub(MOST_TRAINED)..].to_vec());
            let settings = PathSettings::default();

            if bars.len() < settings.fewest_bars() {
                let title = state
                    .snapshot
                    .source
                    .as_ref()
                    .map(Source::title)
                    .unwrap_or_default();

                if let Some(trained) = &state.forecaster {
                    let replayed = trained.replayed(&bars);

                    let note = format!(
                        "{} candles are too few to train on; the networks trained on {} read them.",
                        bars.len(),
                        state.snapshot.trained_on
                    );

                    state.forecaster = Some(replayed);
                    state.snapshot.notes.push(note);
                    state.snapshot.work = Work::Idle;
                } else {
                    state.snapshot.work = Work::Failed(format!(
                        "{title} has {} candles; the networks need {} to train. Watch or load a longer series first, or use the language model.",
                        bars.len(),
                        settings.fewest_bars()
                    ));
                }

                derive(&mut state);
                drop(state);
                self.tell();

                return;
            }

            state.training = true;
            state.snapshot.work = Work::Training(0.0);
            bump(&mut state);

            (bars, settings)
        };

        self.tell();

        let market = self.clone();

        self.inner.runtime.spawn_blocking(move || {
            let progress = |fraction: f32| {
                let mut state = market.state();

                if state.generation == generation {
                    state.snapshot.work = Work::Training(fraction);
                    bump(&mut state);
                }

                drop(state);
                market.notice();
            };

            let trained = Forecaster::train(&bars, &settings, &progress);

            {
                let mut state = market.state();

                state.training = false;

                if state.generation != generation {
                    return;
                }

                match trained {
                    Ok((mut forecaster, report)) => {
                        // Bars that closed while it trained.
                        let latest = forecaster.latest_time_ms().unwrap_or(i64::MIN);

                        for bar in state
                            .snapshot
                            .bars
                            .iter()
                            .filter(|bar| bar.time_ms > latest)
                        {
                            forecaster.advance(*bar);
                        }

                        state.forecaster = Some(forecaster);
                        state.snapshot.report = Some(Arc::new(report));
                        state.snapshot.trained_on = format!(
                            "{} {}",
                            state
                                .snapshot
                                .source
                                .as_ref()
                                .map(Source::title)
                                .unwrap_or_default(),
                            state.snapshot.interval.label()
                        );
                        state.snapshot.since_training = 0;
                        state.snapshot.work = Work::Idle;

                        record(&mut state);
                    }
                    Err(error) => {
                        state.snapshot.work = Work::Failed(error.to_string());
                    }
                }

                derive(&mut state);
            }

            #[cfg(feature = "desktop")]
            market.save_model(generation);
            market.tell();
        });
    }

    /// Tells the screen of a tick, at most four times a second.
    fn notice(&self) {
        let due = {
            let mut state = self.state();

            if state.last_notice.elapsed() >= TICK_NOTICE {
                state.last_notice = Instant::now();
                true
            } else {
                false
            }
        };

        if due {
            self.tell();
        }
    }

    // The live feed ----------------------------------------------------

    fn follow(
        &self,
        generation: u64,
        credentials: Credentials,
        instrument: Instrument,
        interval: Interval,
    ) {
        let (stop, stopped) = watch::channel(false);

        {
            let mut state = self.state();

            if state.generation != generation {
                return;
            }

            state.stop_feed = Some(stop);
        }

        let market = self.clone();
        let session = instrument.segment.session();
        let id: u32 = instrument.security_id.parse().unwrap_or_default();

        let news = {
            let market = market.clone();

            move |news: FeedNews| market.hear(generation, id, session, interval, news)
        };

        self.inner
            .runtime
            .spawn(feed::watch(credentials, instrument, stopped.clone(), news));
        self.start_depth();

        // Closes a candle whose time is up even when no tick comes after it.
        let mut stopped = stopped;

        self.inner.runtime.spawn(async move {
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                    _ = stopped.changed() => return,
                }

                if *stopped.borrow() {
                    return;
                }

                let closed = {
                    let mut state = market.state();

                    if state.generation != generation {
                        return;
                    }

                    close_due(&mut state, session, interval, clock::now_ms())
                };

                if closed {
                    market.after_close(generation);
                }
            }
        });
    }

    fn hear(&self, generation: u64, id: u32, session: Session, interval: Interval, news: FeedNews) {
        let closed = {
            let mut state = self.state();

            if state.generation != generation {
                return;
            }

            match news {
                FeedNews::Connecting => {
                    state.snapshot.feed = FeedState::Connecting;
                    bump(&mut state);
                    false
                }
                FeedNews::Live => {
                    state.snapshot.feed = FeedState::Live;
                    bump(&mut state);
                    false
                }
                FeedNews::Dropped(reason) => {
                    state.snapshot.feed = FeedState::Retrying(reason);
                    bump(&mut state);
                    false
                }
                FeedNews::Stopped(reason) => {
                    state.snapshot.feed = FeedState::Stopped(reason);
                    bump(&mut state);
                    false
                }
                FeedNews::Packet(Packet::PreviousClose { security_id, close }) => {
                    if security_id == id && close > 0.0 {
                        state.snapshot.now.previous_close = Some(close);
                        refresh_now(&mut state);
                    }
                    false
                }
                FeedNews::Packet(Packet::Tick(tick)) if tick.security_id == id => {
                    #[cfg(feature = "desktop")]
                    if let Some(learning) = &state.learning {
                        learning.recorder.record(serde_json::json!({"received_ms":clock::now_ms(),"kind":"tick","source":state.snapshot.source,"tick":tick}));
                    }

                    if !tick.bids.is_empty()
                        && (state.depth_mode == DepthMode::Five
                            || state.snapshot.depth.coverage < 20)
                    {
                        let now = clock::now_ms();
                        state
                            .snapshot
                            .depth
                            .set_side(true, tick.bids.clone(), now, 5);
                        state
                            .snapshot
                            .depth
                            .set_side(false, tick.asks.clone(), now, 5);
                        state.snapshot.depth.status = "Full feed · 5 levels".into();
                    }
                    let closed = tick_in(&mut state, &tick, session, interval);
                    refresh_flow(&mut state.snapshot, clock::now_ms());
                    closed
                }
                FeedNews::Packet(_) => false,
            }
        };

        if closed {
            self.after_close(generation);
        } else {
            self.notice();
        }
        #[cfg(feature = "desktop")]
        self.observe_desktop();
    }

    /// A candle has closed: record forecasts, mark old ones, perhaps
    /// train again or ask the model.
    fn after_close(&self, generation: u64) {
        let (retrain, ask) = {
            let mut state = self.state();

            if state.generation != generation {
                return;
            }

            record(&mut state);
            state.ledger.save();
            derive(&mut state);

            (
                state.snapshot.since_training >= RETRAIN_AFTER && !state.training,
                state.snapshot.brain == Brain::Llm,
            )
        };

        self.tell();

        #[cfg(feature = "desktop")]
        {
            let market = self.clone();
            self.inner.runtime.spawn_blocking(move || {
                market.save_model(generation);
                market.remember_outcomes();
            });
        }
        if retrain {
            self.train(generation);
        }

        if ask {
            self.ask_llm(false);
        }
    }

    // The language model -------------------------------------------------

    /// Asks the language model for the next ten candles, unless it has
    /// already been asked at the latest bar (or `again`).
    pub fn ask_llm(&self, again: bool) {
        let (oracle, prompt, picture, last_close, generation, made_ms, axis_wanted) = {
            let mut state = self.state();

            let Some(oracle) = state.oracle.clone() else {
                state.snapshot.llm.asking =
                    Asking::Failed("No model is set up: choose one in Settings.".to_string());
                bump(&mut state);
                drop(state);
                self.tell();

                return;
            };

            let bars = state.snapshot.bars.clone();

            let Some(last) = bars.last().copied() else {
                return;
            };

            if state.snapshot.llm.asking == Asking::Asking
                || (!again && state.snapshot.llm.made_ms == last.time_ms)
            {
                return;
            }

            let indicators = &state.snapshot.indicators;
            let symbol = state
                .snapshot
                .source
                .as_ref()
                .map(Source::title)
                .unwrap_or_default();
            let relative = matches!(state.snapshot.source, Some(Source::Picture { .. }))
                && state.picture_prices.is_none();

            let prompt = llm::prompt(&llm::Brief {
                symbol: &symbol,
                interval: state.snapshot.interval.label(),
                bars: &bars,
                rsi: Indicators::last(&indicators.rsi),
                vwap: Indicators::last(&indicators.vwap),
                atr: Indicators::last(&indicators.atr),
                support: state.snapshot.levels.support(),
                resistance: state.snapshot.levels.resistance(),
                with_picture: state.picture.is_some(),
                relative_prices: relative,
            });

            state.snapshot.llm.asking = Asking::Asking;
            bump(&mut state);

            (
                oracle,
                prompt,
                state.picture.clone(),
                last.close,
                state.generation,
                last.time_ms,
                relative,
            )
        };

        self.tell();

        let market = self.clone();

        self.inner.runtime.spawn(async move {
            let began = Instant::now();
            let answer = oracle.ask(prompt, picture).await;
            let spent = began.elapsed().as_secs_f64() * 1e3;

            let rescale = {
                let mut state = market.state();

                if state.generation != generation {
                    return;
                }

                let llm = &mut state.snapshot.llm;

                llm.calls += 1;
                llm.total_ms += spent;

                let read = answer.and_then(|text| llm::read(&text, last_close));
                let mut rescale = None;

                match read {
                    Ok(reading) => {
                        llm.asking = Asking::Idle;
                        llm.candles = reading.candles;
                        llm.bias = reading.bias;
                        llm.reason = reading.reason;
                        llm.made_ms = made_ms;

                        if axis_wanted {
                            rescale = reading.axis;
                        }
                    }
                    Err(error) => {
                        llm.failures += 1;
                        llm.asking = Asking::Failed(error);
                    }
                }

                record(&mut state);
                derive(&mut state);

                rescale
            };

            market.tell();

            // The model read the axis: read the picture again with real
            // prices, and ask again on that scale.
            if let Some((high, low)) = rescale {
                let picture = market.state().picture.clone();
                let name = match market.state().snapshot.source.clone() {
                    Some(Source::Picture { name }) => name,
                    _ => return,
                };

                if let Some(picture) = picture {
                    let _ = market.load_picture(&name, picture.bytes, Some((high, low)));
                }
            }
        });
    }

    // Outcomes ---------------------------------------------------------

    /// Forgets the recorded forecasts of the instrument watched, or all.
    pub fn clear_outcomes(&self, everything: bool) {
        {
            let mut state = self.state();
            let key = state.snapshot.source.as_ref().map(Source::key);

            state
                .ledger
                .clear(if everything { None } else { key.as_deref() });
            state.ledger.save();
            derive(&mut state);
        }

        self.tell();
    }

    /// How every network has done on every instrument.
    pub fn all_tallies(&self) -> Vec<Tally> {
        self.state().ledger.tallies(None)
    }
}

fn bump(state: &mut State) {
    state.snapshot.revision += 1;
}

fn read_json<T: for<'a> Deserialize<'a>>(file: &Path) -> Option<T> {
    serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()
}

pub(crate) fn write_json<T: Serialize>(file: &Path, value: &T, private: bool) {
    let Ok(text) = serde_json::to_string(value) else {
        return;
    };

    let mut options = std::fs::OpenOptions::new();

    options.write(true).create(true).truncate(true);

    // The token acts for the account.
    #[cfg(unix)]
    if private {
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    }

    #[cfg(not(unix))]
    let _ = private;

    if let Ok(mut opened) = options.open(file) {
        let _ = std::io::Write::write_all(&mut opened, text.as_bytes());
    }
}

fn media_type(bytes: &[u8]) -> &'static str {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [0xFF, 0xD8, ..] => "image/jpeg",
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'E',
            b'B',
            b'P',
            ..,
        ] => "image/webp",
        _ => "image/png",
    }
}

fn median_gap(bars: &[Bar]) -> i64 {
    let mut gaps: Vec<i64> = bars
        .windows(2)
        .map(|two| two[1].time_ms - two[0].time_ms)
        .collect();

    if gaps.is_empty() {
        return Interval::M5.ms();
    }

    gaps.sort_unstable();
    gaps[gaps.len() / 2]
}

/// Takes off the last bar when it is the candle still forming now.
fn split_forming(
    mut bars: Vec<Bar>,
    interval: Interval,
    session: Session,
    now: i64,
) -> (Vec<Bar>, Option<Bar>) {
    let forming_start = bucket(interval, session, now);

    let forming = (session.is_open(now)
        && bars.last().is_some_and(|bar| bar.time_ms >= forming_start))
    .then(|| bars.pop())
    .flatten();

    (bars, forming)
}

fn bucket(interval: Interval, session: Session, time_ms: i64) -> i64 {
    match interval.minutes() {
        Some(minutes) => session.bucket(time_ms, i64::from(minutes)),
        None => clock::at_ist(clock::ist_day(time_ms), 0),
    }
}

fn next_time(interval: Interval, session: Option<Session>, time_ms: i64, gap_ms: i64) -> i64 {
    match (session, interval.minutes()) {
        (Some(session), Some(minutes)) => session.next(time_ms, i64::from(minutes)),
        (Some(_), None) => clock::next_weekday(time_ms),
        (None, _) => time_ms + gap_ms,
    }
}

/// Folds a tick into the forming candle. Returns whether a candle closed.
fn tick_in(state: &mut State, tick: &Tick, session: Session, interval: Interval) -> bool {
    let now = clock::now_ms();
    let time = feed::trade_time_ms(tick.trade_time, now);

    if tick.last_price <= 0.0 {
        return false;
    }

    let mut closed = false;
    let start = bucket(interval, session, time);

    if let Some(forming) = state.snapshot.forming
        && start > forming.time_ms
    {
        close_forming(state);
        closed = true;
    }

    let volume_today = tick.volume;

    if state.volume_at_start.is_none() {
        state.volume_at_start = volume_today;
    }

    let price = tick.last_price;

    let forming = state.snapshot.forming.get_or_insert(Bar {
        time_ms: start,
        open: price,
        high: price,
        low: price,
        close: price,
        volume: 0.0,
    });

    forming.high = forming.high.max(price);
    forming.low = forming.low.min(price);
    forming.close = price;

    if let (Some(today), Some(at_start)) = (volume_today, state.volume_at_start) {
        forming.volume = (today - at_start).max(0.0);
    }

    let now_numbers = &mut state.snapshot.now;

    now_numbers.tick_ms = Some(now);

    if let Some(average) = tick.average_price.filter(|price| *price > 0.0) {
        now_numbers.vwap = Some(average);
    }

    if volume_today.is_some() {
        now_numbers.volume = volume_today;
    }

    if tick.day_high.is_some_and(|high| high > 0.0) {
        now_numbers.day_high = tick.day_high;
        now_numbers.day_low = tick.day_low;
    }

    if tick.buy_quantity.is_some() {
        now_numbers.bid_quantity = tick.buy_quantity;
        now_numbers.ask_quantity = tick.sell_quantity;
    }

    if tick.open_interest.is_some() {
        now_numbers.open_interest = tick.open_interest;
    }

    if !tick.bids.is_empty() {
        apply_depth(now_numbers, &tick.bids, &tick.asks);
    }

    refresh_now(state);

    closed
}

/// Closes the forming candle when its time is up and the next has not
/// begun with a tick. Returns whether one closed.
fn close_due(state: &mut State, session: Session, interval: Interval, now: i64) -> bool {
    let Some(forming) = state.snapshot.forming else {
        return false;
    };

    let ends = next_time(interval, Some(session), forming.time_ms, interval.ms());

    // A little grace for the last trades of the candle to arrive.
    if now < ends + 3_000 {
        return false;
    }

    close_forming(state);
    refresh_now(state);

    true
}

fn close_forming(state: &mut State) {
    let Some(bar) = state.snapshot.forming.take() else {
        return;
    };

    let mut bars = state.snapshot.bars.as_ref().clone();

    if bars.last().is_some_and(|last| last.time_ms >= bar.time_ms) {
        return;
    }

    bars.push(bar);
    state.snapshot.bars = Arc::new(bars);
    state.snapshot.since_training += 1;
    state.volume_at_start = state.snapshot.now.volume;

    if let Some(forecaster) = &mut state.forecaster {
        forecaster.advance(bar);
    }
}

fn apply_depth(now: &mut Now, bids: &[Level], asks: &[Level]) {
    now.bid = bids
        .first()
        .map(|level| level.price)
        .filter(|price| *price > 0.0);
    now.ask = asks
        .first()
        .map(|level| level.price)
        .filter(|price| *price > 0.0);
    now.spread = match (now.bid, now.ask) {
        (Some(bid), Some(ask)) => Some(ask - bid),
        _ => None,
    };
}

fn apply_quote(now: &mut Now, quote: &Quote) {
    now.price = quote.last_price;
    now.previous_close = quote.previous_close;
    now.vwap = quote.average_price;
    now.volume = Some(quote.volume).filter(|volume| *volume > 0.0);
    now.day_high = Some(quote.high).filter(|high| *high > 0.0);
    now.day_low = Some(quote.low).filter(|low| *low > 0.0);
    now.bid_quantity = Some(quote.buy_quantity).filter(|quantity| *quantity > 0.0);
    now.ask_quantity = Some(quote.sell_quantity).filter(|quantity| *quantity > 0.0);
    now.open_interest = quote.open_interest;

    apply_depth(now, &quote.bids, &quote.asks);
}

/// The price-dependent numbers of `now`, after a tick or a new bar.
fn refresh_now(state: &mut State) {
    let snapshot = &mut state.snapshot;
    let last = snapshot.forming.or_else(|| snapshot.bars.last().copied());

    let Some(last) = last else {
        return;
    };

    let now = &mut snapshot.now;

    now.price = last.close;

    if now.previous_close.is_none() && snapshot.interval.is_intraday() {
        let today = clock::ist_day(last.time_ms);

        now.previous_close = snapshot
            .bars
            .iter()
            .rev()
            .find(|bar| clock::ist_day(bar.time_ms) < today)
            .map(|bar| bar.close);
    } else if now.previous_close.is_none() {
        let count = snapshot.bars.len();

        now.previous_close = match snapshot.forming {
            Some(_) => snapshot.bars.last().map(|bar| bar.close),
            None if count >= 2 => Some(snapshot.bars[count - 2].close),
            None => None,
        };
    }

    if let Some(previous) = now.previous_close {
        now.change = Some(now.price - previous);
        now.change_pct = Some((now.price / previous - 1.0) * 100.0);
    }

    now.buy_share = match (now.bid_quantity, now.ask_quantity) {
        (Some(buy), Some(sell)) if buy + sell > 0.0 => Some(buy / (buy + sell)),
        _ => None,
    };

    bump(state);
}

/// Writes down every brain's forecast made at the latest closed bar, and
/// marks earlier ones whose bar has closed. Only for live instruments:
/// a file's or a picture's future is not coming.
fn record(state: &mut State) {
    let Some(source) = state.snapshot.source.clone() else {
        return;
    };

    if !source.is_live() {
        return;
    }

    let key = source.key();
    let interval = state.snapshot.interval.label().to_string();
    let bars = state.snapshot.bars.clone();

    let Some(last) = bars.last().copied() else {
        return;
    };

    let session = match &source {
        Source::Dhan { instrument } => Some(instrument.segment.session()),
        _ => None,
    };

    let mut paths: Vec<(String, Vec<PredictedCandle>)> = Vec::new();

    if let Some(forecaster) = &state.forecaster
        && forecaster.latest_time_ms() == Some(last.time_ms)
    {
        for net in Net::ALL {
            paths.push((net.short().to_string(), forecaster.path(net).candles));
        }
    }

    if state.snapshot.llm.made_ms == last.time_ms && !state.snapshot.llm.candles.is_empty() {
        paths.push(("LLM".to_string(), state.snapshot.llm.candles.clone()));
    }

    let interval_kind = state.snapshot.interval;

    for (source_name, candles) in paths {
        for step in RECORDED_STEPS {
            let Some(candle) = candles.iter().find(|candle| candle.step == step) else {
                continue;
            };

            let mut due = last.time_ms;

            for _ in 0..step {
                due = next_time(interval_kind, session, due, interval_kind.ms());
            }

            let banded = source_name != "LLM";

            state.ledger.record(Prediction {
                instrument: key.clone(),
                symbol: source.title(),
                interval: interval.clone(),
                source: source_name.clone(),
                made_ms: last.time_ms,
                from: last.close,
                step,
                due_ms: due,
                close: candle.close,
                low: banded.then_some(candle.close_low),
                high: banded.then_some(candle.close_high),
                outcome: None,
            });
        }
    }

    state.ledger.resolve(&key, &interval, &bars);
}

/// Works out everything shown from the bars, the forecaster and the
/// chosen brain.
fn derive(state: &mut State) {
    let snapshot = &mut state.snapshot;
    let bars = snapshot.bars.clone();
    let intraday = snapshot.interval.is_intraday();

    snapshot.indicators = Arc::new(Indicators::of(&bars, intraday));
    snapshot.levels = indicators::levels(&bars, intraday, 150, 3);

    let indicators = snapshot.indicators.clone();

    {
        let now = &mut snapshot.now;

        now.rsi = Indicators::last(&indicators.rsi);
        now.ema_fast = Indicators::last(&indicators.ema_fast);
        now.ema_slow = Indicators::last(&indicators.ema_slow);
        now.atr = Indicators::last(&indicators.atr);

        if now.vwap.is_none() || !matches!(snapshot.source, Some(Source::Dhan { .. })) {
            now.vwap = Indicators::last(&indicators.vwap);
        }
    }

    let candles: Vec<PredictedCandle> = match snapshot.brain.net() {
        Some(net) => state
            .forecaster
            .as_ref()
            .map(|forecaster| forecaster.path(net).candles)
            .unwrap_or_default(),
        None => snapshot.llm.candles.clone(),
    };

    if let Some(forecaster) = &state.forecaster {
        snapshot.account = forecaster.account();
        snapshot.spikes = forecaster.spikes();
        snapshot.networks = [Net::Liquid, Net::Dendritic, Net::Spiking, Net::Ensemble]
            .into_iter()
            .map(|net| (net.short().to_string(), forecaster.path(net).candles))
            .collect();
    }

    let session = match &snapshot.source {
        Some(Source::Dhan { instrument }) => Some(instrument.segment.session()),
        _ => None,
    };

    snapshot.rows = rows(&bars, &candles, snapshot.interval, session, intraday);

    snapshot.plan = bars.last().map(|last| {
        let price = snapshot.forming.map_or(last.close, |bar| bar.close);
        let close = |step: usize| {
            snapshot
                .rows
                .iter()
                .find(|row| row.step == step)
                .map(|row| row.close)
        };

        let accuracy_10 = snapshot
            .brain
            .net()
            .and_then(|net| snapshot.report.as_ref()?.of(net)?.at(10)?.accuracy);

        plan::plan(
            &Reading {
                price,
                rsi: snapshot.now.rsi,
                vwap: snapshot.now.vwap,
                ema_fast: snapshot.now.ema_fast,
                ema_slow: snapshot.now.ema_slow,
                atr: snapshot.now.atr,
                close_1: close(1),
                close_5: close(5),
                close_10: close(10),
                path_low: snapshot.rows.iter().map(|row| row.low).reduce(f64::min),
                path_high: snapshot.rows.iter().map(|row| row.high).reduce(f64::max),
                by: snapshot
                    .rows
                    .last()
                    .map(|row| time_label(row.time_ms, intraday))
                    .unwrap_or_default(),
                source: snapshot.brain.short().to_string(),
                accuracy_10,
            },
            &snapshot.levels,
        )
    });

    let key = snapshot.source.as_ref().map(Source::key);

    snapshot.outcomes = key
        .as_deref()
        .map(|key| state.ledger.latest(Some(key), 300))
        .unwrap_or_default();
    snapshot.tallies = key
        .as_deref()
        .map(|key| state.ledger.tallies(Some(key)))
        .unwrap_or_default();

    refresh_now(state);
}

/// A time as the forecast table shows it.
pub fn time_label(time_ms: i64, intraday: bool) -> String {
    if intraday {
        clock::time_text(time_ms)
    } else {
        clock::day_text(time_ms)
    }
}

/// The forecast table: each candle ahead with its time, and the RSI and
/// VWAP as they would be if it came as forecast.
fn rows(
    bars: &[Bar],
    candles: &[PredictedCandle],
    interval: Interval,
    session: Option<Session>,
    intraday: bool,
) -> Vec<Row> {
    let Some(last) = bars.last().copied() else {
        return Vec::new();
    };

    if candles.is_empty() {
        return Vec::new();
    }

    let gap = median_gap(bars);
    let mut time = last.time_ms;
    let mut extended: Vec<Bar> = bars[bars.len().saturating_sub(400)..].to_vec();
    let kept = extended.len();

    let mut times = Vec::new();

    for candle in candles {
        time = next_time(interval, session, time, gap);
        times.push(time);

        extended.push(Bar {
            time_ms: time,
            open: candle.open,
            high: candle.high,
            low: candle.low,
            close: candle.close,
            volume: candle.volume,
        });
    }

    let indicators = Indicators::of(&extended, intraday);

    candles
        .iter()
        .enumerate()
        .map(|(at, candle)| Row {
            step: candle.step,
            time_ms: times[at],
            open: candle.open,
            high: candle.high,
            low: candle.low,
            close: candle.close,
            volume: candle.volume,
            close_low: candle.close_low,
            close_high: candle.close_high,
            change_pct: (candle.close / last.close - 1.0) * 100.0,
            rsi: indicators.rsi[kept + at],
            vwap: indicators.vwap[kept + at],
            turnover: candle.close * candle.volume,
        })
        .collect()
}

#[cfg(test)]
mod tests;

fn refresh_flow(snapshot: &mut Snapshot, now: i64) {
    if snapshot
        .flow_history
        .last()
        .is_some_and(|last| now - last.time_ms < 500)
    {
        return;
    }
    let sample = crate::flow::sample(
        &snapshot.depth,
        snapshot.now.volume,
        snapshot.flow_history.last(),
        now,
    );
    snapshot.flow_history.push(sample);
    if snapshot.flow_history.len() > 240 {
        snapshot.flow_history.remove(0);
    }
}
