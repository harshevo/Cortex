//! A decision controller and an associative network.
//!
//! The language model decides every step by itself: inspect, edit, run
//! something, or stop. A controller watches the run and learns, from runs
//! whose result was checked, which next action tends to lead to passing
//! checks soonest from a given situation. When it is confident it says so
//! in one line after the step's tool results. It advises; it never acts.
//!
//! The controller feeds the situation's features straight to the learner.
//! A second controller, which first passed them through a spiking network
//! with the size and sparsity of the fruit fly brain, was removed: replayed
//! over saved runs it predicted no better than this one, at 250 MB and
//! milliseconds a step. The network that advises alongside this one now is
//! the dendritic one in `cortex-living`, which is fed events, not features.
//!
//! Beside the controller sits a second, different brain, [`intuition`]:
//! an associative network that learns which files, commands and errors
//! went together in passing runs and brings them to mind when a run
//! reaches a similar place. The controller says what kind of step to take
//! next; the intuition says where.
//!
//! - [`situation`] turns a run's events into features and actions.
//! - [`learner`] learns action values from reward prediction errors.
//! - [`accumulator`] makes actions compete for the decision.
//! - [`controller`] ties them together behind the core traits.
//! - [`intuition`] is the associative network.
//! - [`calibration`] sets how sure a hunch must be, from how hunches have
//!   turned out.
//! - [`bandit`] learns which strategies to give a run's extra attempts.
//! - [`mind`] connects the learning brain served from the `brain` project.

pub mod accumulator;
pub mod bandit;
pub mod calibration;
pub mod controller;
pub mod intuition;
pub mod learner;
pub mod mind;
mod persistence;
pub mod situation;

pub use bandit::StrategyBandit;
pub use calibration::Calibration;
pub use controller::{Controller, DirectEncoder, Encoder};
pub use intuition::Intuition;
pub use mind::Mind;
pub use situation::{Action, Situation};
