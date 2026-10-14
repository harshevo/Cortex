//! The small choices a trading screen offers: which network forecasts,
//! what size of candle, and where the bars come from.

use cortex_liquid::Net;
use serde::{Deserialize, Serialize};

use crate::clock::{DAY_MS, MINUTE_MS};
use crate::dhan::Instrument;

/// What makes the forecast that is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Brain {
    Liquid,
    Dendritic,
    Spiking,
    #[default]
    Ensemble,
    Llm,
}

impl Brain {
    pub const ALL: [Brain; 5] = [
        Brain::Liquid,
        Brain::Dendritic,
        Brain::Spiking,
        Brain::Ensemble,
        Brain::Llm,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Brain::Liquid => "Liquid",
            Brain::Dendritic => "Dendritic",
            Brain::Spiking => "Spiking",
            Brain::Ensemble => "Ensemble",
            Brain::Llm => "Language model",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Brain::Liquid => "LNN",
            Brain::Dendritic => "DANN",
            Brain::Spiking => "SNN",
            Brain::Ensemble => "ENS",
            Brain::Llm => "LLM",
        }
    }

    /// One line on what it is.
    pub fn about(self) -> &'static str {
        match self {
            Brain::Liquid => "Closed-form continuous-time cell that is told how long each gap was",
            Brain::Dendritic => {
                "Grows branches that fire on combinations of past bars, read out linearly"
            }
            Brain::Spiking => "Leaky integrate-and-fire reservoir, its firing read out linearly",
            Brain::Ensemble => "The three networks averaged, weighted by their held-back error",
            Brain::Llm => "The chat model, shown the candles, indicators and any chart picture",
        }
    }

    pub fn net(self) -> Option<Net> {
        match self {
            Brain::Liquid => Some(Net::Liquid),
            Brain::Dendritic => Some(Net::Dendritic),
            Brain::Spiking => Some(Net::Spiking),
            Brain::Ensemble => Some(Net::Ensemble),
            Brain::Llm => None,
        }
    }

    pub fn of(net: Net) -> Self {
        match net {
            Net::Liquid => Brain::Liquid,
            Net::Dendritic => Brain::Dendritic,
            Net::Spiking => Brain::Spiking,
            Net::Ensemble => Brain::Ensemble,
        }
    }

    pub fn from_short(short: &str) -> Option<Self> {
        Brain::ALL.into_iter().find(|brain| brain.short() == short)
    }
}

/// A candle's size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Interval {
    #[serde(rename = "1m")]
    M1,
    #[default]
    #[serde(rename = "5m")]
    M5,
    #[serde(rename = "15m")]
    M15,
    #[serde(rename = "25m")]
    M25,
    #[serde(rename = "1h")]
    H1,
    #[serde(rename = "1d")]
    Day,
}

impl Interval {
    pub const ALL: [Interval; 6] = [
        Interval::M1,
        Interval::M5,
        Interval::M15,
        Interval::M25,
        Interval::H1,
        Interval::Day,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Interval::M1 => "1m",
            Interval::M5 => "5m",
            Interval::M15 => "15m",
            Interval::M25 => "25m",
            Interval::H1 => "1h",
            Interval::Day => "1D",
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        Interval::ALL
            .into_iter()
            .find(|interval| interval.label() == label)
    }

    /// Minutes, as Dhan's intraday endpoint takes them; `None` for days.
    pub fn minutes(self) -> Option<u32> {
        match self {
            Interval::M1 => Some(1),
            Interval::M5 => Some(5),
            Interval::M15 => Some(15),
            Interval::M25 => Some(25),
            Interval::H1 => Some(60),
            Interval::Day => None,
        }
    }

    pub fn ms(self) -> i64 {
        self.minutes()
            .map_or(DAY_MS, |minutes| i64::from(minutes) * MINUTE_MS)
    }

    /// Days of history fetched: enough bars to train on, not so many that
    /// fetching takes long.
    pub fn history_days(self) -> i64 {
        match self {
            Interval::M1 => 7,
            Interval::M5 => 30,
            Interval::M15 => 75,
            Interval::M25 => 120,
            Interval::H1 => 240,
            Interval::Day => 365 * 4,
        }
    }

    pub fn is_intraday(self) -> bool {
        self != Interval::Day
    }

    /// The interval nearest a gap between bars.
    pub fn nearest(gap_ms: i64) -> Self {
        Interval::ALL
            .into_iter()
            .min_by_key(|interval| (interval.ms() - gap_ms).abs())
            .unwrap_or(Interval::M5)
    }
}

/// Where the bars come from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    Dhan { instrument: Instrument },
    File { name: String },
    Picture { name: String },
}

impl Source {
    /// What it is called on screen.
    pub fn title(&self) -> String {
        match self {
            Source::Dhan { instrument } => instrument.symbol.clone(),
            Source::File { name } | Source::Picture { name } => name.clone(),
        }
    }

    /// The key forecasts are filed under.
    pub fn key(&self) -> String {
        match self {
            Source::Dhan { instrument } => instrument.key(),
            Source::File { name } => format!("file:{name}"),
            Source::Picture { name } => format!("picture:{name}"),
        }
    }

    pub fn is_live(&self) -> bool {
        matches!(self, Source::Dhan { .. })
    }
}
