//! Remote control of the desktop app from a phone.
//!
//! The desktop app is the host: the agent, its tools and its services run
//! there. A phone is a second window onto it. Both build the conversation
//! from the same events with the same [`transcript`], so they cannot
//! disagree about what happened.
//!
//! The two are joined by an encrypted connection dialed by public key
//! ([`link`]), which goes directly when it can and through a relay when
//! it cannot, so neither side opens a port. A phone is let in once it has
//! shown a pairing code read from the desktop's screen ([`pairing`]).

pub mod link;
pub mod mirror;
pub mod pairing;
pub mod stats;
pub mod transcript;
pub mod wire;

pub use link::{Client, Host};
pub use mirror::Mirror;
pub use pairing::{Device, Ticket};
