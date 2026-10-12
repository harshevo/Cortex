//! The router.
//!
//! A plain agent spends its first steps finding its way: listing files,
//! searching, opening the wrong file. Each of those steps is a full model
//! call. The router looks at the task before the model does and gathers
//! the context this kind of task usually needs, so the model starts closer
//! to the answer.
//!
//! This version decides with hand-written rules. Every decision is logged
//! with the run, so that a small classifier can later be trained on which
//! decisions led to short, successful runs and replace the rules.
//!
//! - [`rules`] classifies the task and picks the context sources.
//! - [`symbols`] finds the code names a task mentions.
//! - [`context`] gathers the chosen context into one briefing.
//! - [`spread`] ranks the files a task is probably about, by its words and
//!   the import graph.
//! - [`map`] is the project map, which every task gets when it is switched on.

pub mod context;
pub mod map;
pub mod rules;
pub mod spread;
pub mod symbols;

pub use context::ContextRouter;
pub use map::ProjectMap;
pub use rules::{Available, Route, Source, TaskKind};
pub use spread::LikelyFiles;
