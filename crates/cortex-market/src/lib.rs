//! Market data and forecasts for the trading screens.
//!
//! - [`dhan`]: the Dhan broker's data API and live feed.
//! - [`indicators`]: RSI, VWAP, moving averages, ATR and price levels.
//! - [`plan`]: what to do if the price holds its range or breaks out.
//! - [`outcomes`]: every live forecast, and whether it came true.
//! - [`picture`]: candles read off a picture of a chart.
//! - [`llm`]: a language model asked for the next candles.
//! - [`market`]: all of it for one instrument, run in the background.
//! - [`standin`]: a pretend Dhan on this machine, for trying the screens
//!   without an account.
//!
//! Nothing here places orders or gives advice: the forecasts and plans
//! are shown beside a measured record of how such forecasts did.

pub mod clock;
pub mod dhan;
pub mod indicators;
pub mod kinds;
pub mod llm;
pub mod market;
pub mod oracle;
pub mod outcomes;
pub mod picture;
pub mod plan;
pub mod standin;

pub use dhan::{Credentials, Instrument, Profile, Quote, Segment};
pub use kinds::{Brain, Interval, Source};
pub use llm::{Answer, Oracle, Picture};
pub use market::{Asking, Connection, FeedState, LlmState, Market, Now, Row, Snapshot, Work};
pub use oracle::ProviderOracle;
pub use outcomes::{Prediction, Tally};
pub use plan::{Bias, Plan, Scenario, Tone, price_text};
