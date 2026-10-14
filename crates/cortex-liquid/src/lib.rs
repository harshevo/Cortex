//! Reading price bars with two small networks, and saying honestly
//! whether either predicts anything.
//!
//! - [`bars`]: a bar, and a tolerant reader of CSV files.
//! - [`features`]: the scale-free numbers each bar is described by, all
//!   from that bar and earlier ones.
//! - [`cfc`]: the liquid network, a closed-form continuous-time cell
//!   that is told how much time passed between bars, trained by
//!   hand-written backpropagation.
//! - [`regime`]: the dendritic network from `cortex-living`, fed each bar
//!   as a few words, whose expectations of the next bar become extra
//!   inputs to the liquid network.
//! - [`analysis`]: trains both on the first part of a series, scores
//!   them and three baselines on the held-out last part, and gives a
//!   verdict that is "no edge" unless the evidence is strong.
//!
//! - [`path`]: the next ten candles from the liquid, the dendritic and
//!   the spiking network ([`spiking`]), each measured on held-out bars.
//!
//! - [`desk`]: one series being looked at by an app, analysed on a
//!   thread of its own.
//!
//! Start with [`parse_csv`] and [`analyse`], or [`Model::train`] to keep
//! the trained model and feed it new bars later.
//!
//! Apart from [`desk`], everything is plain synchronous code on one
//! thread with no access to the disk, the network or a screen, so it can
//! be called from a background thread of an app. Nothing here is advice: the output is a
//! forecast and a measurement of how such forecasts did on past bars.

pub mod abilities;
pub mod analysis;
pub mod bars;
pub mod cfc;
pub mod desk;
mod error;
pub mod features;
pub mod path;
pub mod regime;
pub mod ridge;
pub mod rng;
pub mod spiking;
pub mod stats;
pub mod synthetic;

#[cfg(test)]
mod tests;

pub use analysis::{
    Analysis, Direction, Edge, Forecast, Model, Predictor, Progress, Score, Series, Settings,
    Stage, Timings, Update, Verdict, analyse,
};
pub use bars::{Bar, Parsed, parse_csv, parse_time};
pub use cfc::EpochLoss;
pub use desk::{Candle, Desk, Status};
pub use error::Error;
pub use path::{
    Forecaster, Net, NetMetrics, Path, PathReport, PathSettings, PredictedCandle, STEPS, StepScore,
};
pub use regime::{Account, Expectation, Growth, ImaginedStep};
