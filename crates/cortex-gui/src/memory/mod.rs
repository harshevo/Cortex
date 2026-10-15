//! The agent's memory, as the app shows it.
//!
//! The memory itself belongs to the engine (`cortex_brain::memory`): it records
//! each run, consolidates it into an episode, and promotes what episodes
//! have in common to long-term facts. This module only reads that memory
//! and arranges it for drawing:
//!
//! - [`reader`] reads episodes and facts from the engine's store.

pub mod reader;
pub mod seed;
pub mod words;

pub use reader::{MemoryReader, Remembered};
