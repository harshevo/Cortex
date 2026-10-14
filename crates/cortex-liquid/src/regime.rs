//! The dendritic network's side: each bar is put into words, the words
//! are fed in time order to a [`cortex_living`] network, and what that
//! network then expects of the next bar is handed on as numbers.
//!
//! A bar becomes up to five percepts, one from each of
//!
//! - `move: down-big | down | flat | up | up-big`
//! - `vol: quiet | normal | loud` (left out when the file has no volume)
//! - `range: tight | normal | wide`
//! - `trend: down | none | up`
//! - `osc: low | mid | high`
//!
//! with the edges of the buckets taken from the training part only. The
//! network grows a neuron per percept, and neurons for combinations that
//! recur, and learns on its branches which earlier combinations each
//! `move:` follows. It learns online: by the time it is asked about the
//! next bar it has lived every bar up to now and none after.
//!
//! What comes out for the liquid network is one number per `move:`
//! bucket, how sure the dendritic network is that the next bar is that
//! one (0 when it does not expect it), and its running surprise.
//!
//! What this cannot do: the dendritic network was built to follow an
//! agent's work, where the same things recur exactly. It speaks only
//! when a branch that has been right before is driven again, so on many
//! bars it expects no move at all and the numbers handed on are zeros.
//! It keeps learning on every bar it is shown, including held-out ones
//! as they pass, which uses nothing from the future but does mean its
//! record is a running one, not that of a frozen model.
//!
//! The network's working context (what fired last, what it expects) is
//! not part of its saved form. So a `Regime` is saved as the bucket codes
//! of every bar it has lived and rebuilt by living them again, which
//! gives exactly the network that was saved at the cost of about a
//! twentieth of a millisecond per bar on a laptop.

use cortex_living::network::Network;
use serde::{Deserialize, Serialize};

use crate::features::{self, Row};
use crate::stats::quantile;

pub const MOVES: [&str; 5] = [
    "move: down-big",
    "move: down",
    "move: flat",
    "move: up",
    "move: up-big",
];
const VOLUMES: [&str; 3] = ["vol: quiet", "vol: normal", "vol: loud"];
const RANGES: [&str; 3] = ["range: tight", "range: normal", "range: wide"];
const TRENDS: [&str; 3] = ["trend: down", "trend: none", "trend: up"];
const OSCILLATORS: [&str; 3] = ["osc: low", "osc: mid", "osc: high"];

/// The beginning of the percepts that say which way a bar went.
pub const MOVE: &str = "move: ";

/// Numbers handed to the liquid network for each bar: one per move
/// bucket, and the surprise.
pub const OUTLOOK: usize = MOVES.len() + 1;

/// How sure the network must be of a move for it to count when the move
/// is not among its few surest expectations. Where the network starts
/// its own threshold for an expectation.
const FORESEEN: f32 = 0.35;

/// Steps the network is asked to imagine ahead for its account.
const IMAGINED: usize = 4;

/// The names of the regimes a bar can be in: its trend and how wide its
/// range is. A bar's regime is an index into this.
pub fn regime_names() -> Vec<String> {
    let mut names = Vec::new();

    for trend in ["falling", "no trend", "rising"] {
        for range in ["tight", "normal", "wide"] {
            names.push(format!("{trend}, {range} range"));
        }
    }

    names
}

/// Which bucket of each kind a bar fell in. `NONE` for volume when the
/// series has none.
pub type Codes = [u8; 5];

const NONE: u8 = u8::MAX;

/// Where the edges of the buckets are, from the training part.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Buckets {
    /// A return below the first is down-big, below the second down,
    /// below the third flat, below the fourth up, otherwise up-big.
    pub moves: [f64; 4],

    /// Absent when no bar in training had a volume.
    pub volume: Option<[f64; 2]>,
    pub range: [f64; 2],
    pub trend: [f64; 2],
    pub oscillator: [f64; 2],

    /// The mean one-bar return of each move bucket in training.
    pub move_means: [f64; 5],
}

/// The bucket `value` falls in: the number of edges it is not below.
fn bucket(value: f64, edges: &[f64]) -> u8 {
    edges.iter().filter(|edge| value >= **edge).count() as u8
}

impl Buckets {
    /// Sets the edges from `rows`, which must be the training part only.
    pub fn fit(rows: &[Row]) -> Self {
        let column = |at: usize| rows.iter().map(|row| row[at]).collect::<Vec<f64>>();

        let pair = |at: usize, low: f64, high: f64| {
            let values = column(at);

            [quantile(&values, low), quantile(&values, high)]
        };

        let returns = column(features::RETURN);

        // The outer tenths are the big moves and the middle three tenths
        // are flat.
        let moves = [0.1, 0.35, 0.65, 0.9].map(|share| quantile(&returns, share));

        let has_volume = rows.iter().any(|row| row[features::VOLUME] != 0.0);

        let mut sums = [0.0; 5];
        let mut counts = [0.0; 5];

        for value in &returns {
            let at = bucket(*value, &moves) as usize;

            sums[at] += value;
            counts[at] += 1.0;
        }

        let mut move_means = [0.0; 5];

        for at in 0..5 {
            if counts[at] > 0.0 {
                move_means[at] = sums[at] / counts[at];
            }
        }

        Self {
            moves,
            volume: has_volume.then(|| pair(features::VOLUME, 0.25, 0.75)),
            range: pair(features::RANGE, 0.25, 0.75),
            trend: pair(features::RETURN_20, 1.0 / 3.0, 2.0 / 3.0),
            oscillator: pair(features::OSCILLATOR, 0.25, 0.75),
            move_means,
        }
    }

    pub fn codes(&self, row: &Row) -> Codes {
        [
            bucket(row[features::RETURN], &self.moves),
            self.volume
                .map_or(NONE, |edges| bucket(row[features::VOLUME], &edges)),
            bucket(row[features::RANGE], &self.range),
            bucket(row[features::RETURN_20], &self.trend),
            bucket(row[features::OSCILLATOR], &self.oscillator),
        ]
    }
}

/// The percepts for a bar's codes.
pub fn percepts(codes: &Codes) -> Vec<String> {
    let mut percepts = vec![MOVES[codes[0] as usize].to_string()];

    if codes[1] != NONE {
        percepts.push(VOLUMES[codes[1] as usize].to_string());
    }

    percepts.push(RANGES[codes[2] as usize].to_string());
    percepts.push(TRENDS[codes[3] as usize].to_string());
    percepts.push(OSCILLATORS[codes[4] as usize].to_string());

    percepts
}

/// The regime a bar's codes put it in, as an index into
/// [`regime_names`].
pub fn regime_of(codes: &Codes) -> u8 {
    codes[3] * 3 + codes[2]
}

/// What the dendritic network makes of the next bar, as numbers.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Outlook {
    /// How sure it is of each move bucket, in the order of [`MOVES`],
    /// from 0 to 1. 0 for a move it does not expect.
    pub moves: [f32; 5],

    /// Its running average of how much of each bar it had not expected,
    /// from 0 to 1.
    pub surprise: f32,
}

impl Outlook {
    /// The numbers handed to the liquid network.
    pub fn row(&self) -> [f64; OUTLOOK] {
        let mut row = [0.0; OUTLOOK];

        for (at, sure) in self.moves.iter().enumerate() {
            row[at] = *sure as f64;
        }

        row[MOVES.len()] = self.surprise as f64;

        row
    }

    /// The move bucket it is surest of and how sure, if it expects any.
    pub fn surest(&self) -> Option<(usize, f32)> {
        let (at, sure) = self
            .moves
            .iter()
            .copied()
            .enumerate()
            .max_by(|left, right| left.1.total_cmp(&right.1))?;

        (sure > 0.0).then_some((at, sure))
    }

    /// Whether it expects the next bar up (`Some(true)`), down, or says
    /// nothing: it expects no move, or expects a flat one.
    pub fn up(&self) -> Option<bool> {
        match self.surest()?.0 {
            0 | 1 => Some(false),
            3 | 4 => Some(true),
            _ => None,
        }
    }
}

/// One thing the dendritic network expects, and how sure it is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Expectation {
    pub label: String,
    pub confidence: f32,
}

/// One step of the way on as the dendritic network imagines it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImaginedStep {
    pub labels: Vec<String>,

    /// How sure the whole path up to this step is.
    pub confidence: f32,
}

/// The dendritic network's own account of what comes after the latest
/// bar, for showing why.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Account {
    /// What the latest bar was, in its words.
    pub now: Vec<String>,

    /// The regime of the latest bar.
    pub regime: String,

    /// Everything it expects of the next bar, surest first. Often empty.
    pub expectations: Vec<Expectation>,

    /// Running on its own expectations for a few bars. Often empty.
    pub imagined: Vec<ImaginedStep>,

    pub surprise: f32,
}

/// How much the dendritic network has grown and what it has got right.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Growth {
    pub neurons: usize,
    pub segments: usize,
    pub synapses: usize,

    /// Bars lived.
    pub moments: u64,

    /// Running share of each bar's percepts it had expected.
    pub expected_right: f32,

    /// The same for always guessing the commonest percepts. It has
    /// learned something only where `expected_right` is above this.
    pub baseline_right: f32,

    /// Running share of what it expected that came.
    pub precision: f32,
    pub surprise: f32,
}

/// The bucket edges, the dendritic network, and every bar it has lived.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(from = "Saved", into = "Saved")]
pub struct Regime {
    buckets: Buckets,
    lived: Vec<Codes>,
    network: Network,
}

/// What is written to a file: enough to rebuild the network exactly.
#[derive(Serialize, Deserialize)]
struct Saved {
    buckets: Buckets,
    lived: Vec<Codes>,
}

impl From<Regime> for Saved {
    fn from(regime: Regime) -> Self {
        Self {
            buckets: regime.buckets,
            lived: regime.lived,
        }
    }
}

impl From<Saved> for Regime {
    fn from(saved: Saved) -> Self {
        let mut regime = Regime::new(saved.buckets);

        for codes in saved.lived {
            // Codes from a damaged file could point outside the tables.
            if codes_are_sound(&codes) {
                regime.live(codes);
            }
        }

        regime
    }
}

fn codes_are_sound(codes: &Codes) -> bool {
    (codes[0] as usize) < MOVES.len()
        && (codes[1] == NONE || codes[1] < 3)
        && codes[2..].iter().all(|code| *code < 3)
}

impl PartialEq for Regime {
    /// Two are equal when they have the same edges and have lived the
    /// same bars, since the network follows from those.
    fn eq(&self, other: &Self) -> bool {
        self.buckets == other.buckets && self.lived == other.lived
    }
}

impl Regime {
    /// A network that has seen nothing.
    pub fn new(buckets: Buckets) -> Self {
        Self {
            buckets,
            lived: Vec::new(),

            // No percept of a market is good or bad news in itself. The
            // one entry, worth nothing, is there because an empty table
            // is replaced with the agent's own.
            network: Network::with_meanings(&[(MOVES[2], 0.0)], &[MOVE]),
        }
    }

    pub fn buckets(&self) -> &Buckets {
        &self.buckets
    }

    /// The codes of every bar lived, oldest first.
    pub fn lived(&self) -> &[Codes] {
        &self.lived
    }

    pub fn network(&self) -> &Network {
        &self.network
    }

    fn live(&mut self, codes: Codes) {
        self.network.live(&percepts(&codes));
        self.lived.push(codes);
    }

    /// The next bar has closed. The network lives it and says what it
    /// now makes of the one after.
    pub fn observe(&mut self, row: &Row) -> Outlook {
        self.live(self.buckets.codes(row));

        self.outlook()
    }

    /// What the network makes of the bar after the latest one it lived.
    pub fn outlook(&self) -> Outlook {
        let mut outlook = Outlook {
            surprise: self.network.meters().surprise,
            ..Outlook::default()
        };

        // The network keeps only its few surest expectations of any
        // kind, and the trend and the oscillator, which change slowly,
        // are nearly always surer than a move. So it is also asked for
        // the move it is surest of, whatever else it expects.
        let foreseen = self.network.foresee(MOVE, FORESEEN);

        for (label, sure) in self.network.expectations().into_iter().chain(foreseen) {
            if let Some(at) = MOVES.iter().position(|name| *name == label) {
                outlook.moves[at] = outlook.moves[at].max(sure);
            }
        }

        outlook
    }

    /// The network's account of what comes next, in its own words.
    pub fn account(&self) -> Account {
        let Some(latest) = self.lived.last() else {
            return Account::default();
        };

        let mut expectations: Vec<Expectation> = self
            .network
            .expectations()
            .into_iter()
            .map(|(label, confidence)| Expectation { label, confidence })
            .collect();

        expectations.sort_by(|left, right| {
            right
                .confidence
                .total_cmp(&left.confidence)
                .then_with(|| left.label.cmp(&right.label))
        });

        Account {
            now: percepts(latest),
            regime: regime_names()[regime_of(latest) as usize].clone(),
            expectations,
            imagined: self
                .network
                .imagine(IMAGINED)
                .into_iter()
                .map(|thought| ImaginedStep {
                    labels: thought.labels,
                    confidence: thought.confidence,
                })
                .collect(),
            surprise: self.network.meters().surprise,
        }
    }

    pub fn growth(&self) -> Growth {
        let meters = self.network.meters();

        Growth {
            neurons: meters.neurons,
            segments: meters.segments,
            synapses: meters.synapses,
            moments: meters.moments,
            expected_right: meters.expected_right,
            baseline_right: meters.baseline_right,
            precision: meters.precision,
            surprise: meters.surprise,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synthetic;

    fn lived(count: usize) -> (Regime, Vec<Outlook>) {
        let bars = synthetic::patterned(count, 4);
        let rows = features::table(&bars);

        let mut regime = Regime::new(Buckets::fit(&rows[..count / 2]));
        let outlooks = rows.iter().map(|row| regime.observe(row)).collect();

        (regime, outlooks)
    }

    #[test]
    fn buckets_split_training_as_the_quantiles_say() {
        let bars = synthetic::random_walk(400, 6);
        let rows = features::table(&bars);
        let buckets = Buckets::fit(&rows);

        let mut counts = [0usize; 5];

        for row in &rows {
            counts[buckets.codes(row)[0] as usize] += 1;
        }

        // A tenth, a quarter, three tenths, a quarter, a tenth.
        for (count, share) in counts.iter().zip([0.1, 0.25, 0.3, 0.25, 0.1]) {
            assert!((*count as f64 / 400.0 - share).abs() < 0.02, "{counts:?}");
        }

        assert!(buckets.move_means[0] < buckets.move_means[1]);
        assert!(buckets.move_means[3] < buckets.move_means[4]);
        assert!(buckets.move_means[0] < 0.0 && buckets.move_means[4] > 0.0);
        assert!(buckets.volume.is_some());
    }

    #[test]
    fn a_series_without_volume_has_no_volume_percept() {
        let mut bars = synthetic::random_walk(120, 6);

        bars.iter_mut().for_each(|bar| bar.volume = 0.0);

        let rows = features::table(&bars);
        let buckets = Buckets::fit(&rows);
        let said = percepts(&buckets.codes(&rows[100]));

        assert_eq!(buckets.volume, None);
        assert_eq!(said.len(), 4);
        assert!(said.iter().all(|percept| !percept.starts_with("vol: ")));
    }

    #[test]
    fn the_network_grows_and_its_outlook_is_within_bounds() {
        let (regime, outlooks) = lived(300);
        let growth = regime.growth();

        assert_eq!(growth.moments, 300);
        assert!(growth.neurons > 14, "{growth:?}");
        assert!(growth.segments > 0 && growth.synapses > 0, "{growth:?}");

        for outlook in &outlooks {
            assert!(outlook.moves.iter().all(|sure| (0.0..=1.0).contains(sure)));
            assert!((0.0..=1.0).contains(&outlook.surprise));
        }

        let account = regime.account();

        assert_eq!(account.now.len(), 5);
        assert!(account.now[0].starts_with(MOVE));
        assert!(regime_names().contains(&account.regime));
    }

    #[test]
    fn what_it_has_lived_never_depends_on_later_bars() {
        let bars = synthetic::patterned(200, 4);
        let rows = features::table(&bars);
        let buckets = Buckets::fit(&rows[..100]);

        let run = |upto: usize| {
            let mut regime = Regime::new(buckets.clone());

            rows[..upto]
                .iter()
                .map(|row| regime.observe(row))
                .collect::<Vec<Outlook>>()
        };

        let short = run(150);
        let long = run(200);

        assert_eq!(short[..], long[..150]);
    }

    #[test]
    fn a_saved_regime_comes_back_expecting_the_same() {
        let (regime, _) = lived(250);

        let saved = serde_json::to_string(&regime).unwrap();
        let mut loaded: Regime = serde_json::from_str(&saved).unwrap();

        assert_eq!(loaded, regime);
        assert_eq!(loaded.outlook(), regime.outlook());
        assert_eq!(loaded.account(), regime.account());
        assert_eq!(loaded.growth(), regime.growth());

        // And goes on the same from there.
        let mut regime = regime;
        let more = features::table(&synthetic::patterned(40, 8));

        for row in &more[20..] {
            assert_eq!(loaded.observe(row), regime.observe(row));
        }
    }

    #[test]
    fn an_outlook_reads_as_a_direction() {
        let outlook = |moves: [f32; 5]| Outlook {
            moves,
            surprise: 0.0,
        };

        assert_eq!(outlook([0.0; 5]).up(), None);
        assert_eq!(outlook([0.0, 0.0, 0.9, 0.2, 0.0]).up(), None);
        assert_eq!(outlook([0.0, 0.3, 0.0, 0.6, 0.0]).up(), Some(true));
        assert_eq!(outlook([0.7, 0.0, 0.0, 0.6, 0.0]).up(), Some(false));
        assert_eq!(outlook([0.0, 0.3, 0.0, 0.6, 0.0]).surest(), Some((3, 0.6)));
    }
}
