//! The one brain.
//!
//! A contextual neural sequence learner, explicit grounded skills,
//! source-linked knowledge and memory, replay, and internal monitoring.
//! Chats, the coding agent and the trading desk reach the same background
//! service. Built-in arithmetic and symbolic inference are identified
//! separately from learning in neural connections.
//!
//! - [`brain`]: the brain itself. Its cortex learns what comes next, from
//!   words and from what happens alike; it keeps memories as the cells
//!   that held them; it speaks, recalls, thinks, imagines, reasons,
//!   follows hunches, reflects, lives streams of experience with value
//!   and attention, advises, and sleeps, replaying, pruning and dreaming.
//! - [`circuit`]: an isolated membrane/dendrite simulator and experimental
//!   circuit-trained operation selector. It uses no pretrained language model.
//! - [`senses`]: how the coding agent's work and a market's bars reach
//!   it, as percepts.
//! - [`memory`]: its explicit memory of past tasks in each repository
//!   (working, episodic and long-term facts), recalled into the agent's
//!   context before a task. This is kept in words, not in weights.
//! - [`link`]: the handle everything else holds: to the brain in the
//!   background service, or to one in this process.
//! - [`protocol`]: what can be asked of it, answered the same way by
//!   either.
//! - [`service`]: the background service that owns the brain, keeps it
//!   learning, sleeping and dreaming while nothing else runs, and the LLM
//!   teacher that teaches it to talk.
//!
//! It never acts. It advises, and only where its own record says it is
//! usually right. None of what it measures of itself is evidence of
//! experience.

pub mod brain;
pub mod circuit;
pub mod link;
pub mod memory;
pub mod protocol;
pub mod senses;
#[cfg(unix)]
pub mod service;

pub use brain::{
    Brain, Engram, Hunch, HunchKind, Imagined, Moment, NeuralSnapshot, Outlook, Recall, Step,
    Thought, Utterance,
};
pub use link::{Counsel, LearningSink, Link};
pub use protocol::Request;
