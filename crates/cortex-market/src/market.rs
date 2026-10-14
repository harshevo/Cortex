//! One instrument being watched: its bars, live from Dhan or loaded from
//! a file or a picture, the networks trained on them, their forecasts,
//! the plan, and the record of how earlier forecasts came out.
//!
//! The work happens on background threads; a screen calls
//! [`Market::snapshot`] whenever it is told something changed, and draws
//! what it gets.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use cortex_liquid::{Account, Bar, Forecaster, Net, PathReport, PathSettings, PredictedCandle};
use serde::{Deserialize, Serialize};
use tokio::runtime::Runtime;
use tokio::sync::watch;

use crate::clock::{self, Session};
use crate::dhan::feed::{self, FeedNews, Packet, Tick};
use crate::dhan::instruments::{self, Instrument};
use crate::dhan::{Credentials, Dhan, Level, Profile, Quote};
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
#[derive(Debug, Clone, PartialEq, Default)]
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
#[derive(Debug, Clone, PartialEq)]
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
}

/// What is kept between runs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Kept {
    instrument: Option<Instrument>,
    interval: Option<Interval>,
    brain: Option<Brain>,
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
}

struct Inner {
    folder: PathBuf,
    runtime: Runtime,
    state: Mutex<State>,
    changed: Box<dyn Fn() + Send + Sync>,
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
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("market")
            .enable_all()
            .build()
            .expect("the market needs its background threads");

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
                }),
                changed: Box::new(changed),
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
        (self.inner.changed)();
    }

    /// Changes whenever anything in the snapshot does.
    pub fn revision(&self) -> u64 {
        self.state().snapshot.revision
    }

    pub fn snapshot(&self) -> Snapshot {
        self.state().snapshot.clone()
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
        };

        drop(state);

        write_json(&self.inner.folder.join("market.json"), &kept, false);
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

        state.generation += 1;
        state.snapshot.source = Some(source);
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
                    tick_in(&mut state, &tick, session, interval)
                }
                FeedNews::Packet(_) => false,
            }
        };

        if closed {
            self.after_close(generation);
        } else {
            self.notice();
        }
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

fn write_json<T: Serialize>(file: &Path, value: &T, private: bool) {
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
