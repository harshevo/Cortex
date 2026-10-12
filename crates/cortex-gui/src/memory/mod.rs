//! The agent's memory, as the app shows it.
//!
//! The memory itself belongs to the engine (`cortex-memory`): it records
//! each run, consolidates it into an episode, and promotes what episodes
//! have in common to long-term facts. This module only reads that memory
//! and arranges it for drawing:
//!
//! - [`reader`] reads episodes and facts from the engine's store.
//! - [`working`] turns the engine's working memory into thoughts.
//! - [`graph`] joins the three tiers into one graph of nodes and links.
//! - [`layout`] places episodes by meaning.

pub mod graph;
pub mod layout;
pub mod reader;
pub mod seed;
pub mod words;
pub mod working;

pub use graph::Region;
pub use reader::{MemoryReader, Remembered};
