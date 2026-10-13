//! Brain-inspired decision controllers.
//!
//! The language model decides every step by itself: inspect, edit, run
//! something, or stop. A controller watches the run and learns, from runs
//! whose result was checked, which next action tends to lead to passing
//! checks soonest from a given situation. When it is confident it says so
//! in one line after the step's tool results. It advises; it never acts.
//!
//! Two controllers share everything except how a situation is encoded:
//!
//! - The **baseline** feeds the situation's features straight to the
//!   learner. It is the conventional controller the other has to beat.
//! - The **fly brain** first passes the features through a spiking network
//!   with the size and sparsity of the adult fruit fly brain (139,255
//!   neurons, about 50 million synapses) and learns from the activity of
//!   its Kenyon cells, the way the fly's mushroom body learns odours.
//!
//! What "fly brain" means here, precisely: the neuron count, synapse
//! count, sparse expansion onto Kenyon cells, and reward-gated learning
//! follow the fly. The wiring is generated, not the measured connectome,
//! unless a connectome file is supplied. Even with the measured wiring,
//! the fly's circuits evolved for flying and feeding; nothing suggests they
//! suit software decisions, and the baseline exists to test exactly that.
//!
//! Beside the controllers sits a second, different brain, [`intuition`]:
//! an associative network that learns which files, commands and errors
//! went together in passing runs and brings them to mind when a run
//! reaches a similar place. The controllers say what kind of step to take
//! next; the intuition says where.
//!
//! - [`situation`] turns a run's events into features and actions.
//! - [`connectome`] holds the wiring, generated or loaded.
//! - [`spiking`] simulates the network.
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
pub mod connectome;
pub mod controller;
pub mod intuition;
pub mod learner;
pub mod mind;
mod persistence;
pub mod situation;
pub mod spiking;

pub use bandit::StrategyBandit;
pub use calibration::Calibration;
pub use connectome::Connectome;
pub use controller::{Controller, DirectEncoder, Encoder, FlyBrainEncoder};
pub use intuition::Intuition;
pub use mind::Mind;
pub use situation::{Action, Situation};
pub use spiking::{Brain, BrainConfig};
