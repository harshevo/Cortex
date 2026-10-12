//! What the window shows about a conversation, built from the agent's
//! events. Replaying a saved trajectory through the same code is how past
//! sessions are shown.

pub mod history;
pub mod stats;
pub mod transcript;

pub use stats::{Cell, Stats};
pub use transcript::{Activity, Doing, Item, Session, Tone, ToolItem, Writing};
