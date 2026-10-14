//! The pieces of the `cortex` command, as a library.
//!
//! The desktop app builds its agent from the same arguments and the same
//! [`features::setup`] as the command line, so a capability switched on
//! with `--with` behaves identically in both.

pub mod approver;
#[cfg(unix)]
pub mod brain_commands;
pub mod args;
pub mod commands;
pub mod console;
pub mod features;
pub mod session;
