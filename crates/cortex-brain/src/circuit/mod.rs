//! Shared recurrent/dendritic circuit with learned operation and language readouts.
//! Exact arithmetic is supplied separately; activity does not establish subjective experience.

mod config;
pub mod dialogue;
pub mod encoding;
pub mod ipc;
pub mod language;
mod network;
mod neuron;
pub mod readout;
pub mod sequence;

pub use config::{Config, Population};
pub use network::{Activity, Live, Network, Snapshot, Synapse};
pub use neuron::{Branch, Neuron};

#[cfg(test)]
mod tests;
