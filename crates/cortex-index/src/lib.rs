//! Code understanding for the agent.
//!
//! Source files are parsed with Tree-sitter into a list of symbols
//! (functions, classes, types and so on) with their line ranges. That lets
//! the agent ask "where is `matchFills` defined?" or "what is in this file?"
//! and then read only the lines it needs, instead of whole files.
//!
//! - [`language`] decides which grammar a file uses and which syntax nodes
//!   count as definitions.
//! - [`symbols`] extracts the symbols of one file.
//! - [`index`] keeps the symbols of a whole project, re-parsing only files
//!   that changed.
//! - [`references`] extracts the imports and name uses of one file.
//! - [`resolve`] turns an import into the project file it refers to.
//! - [`graph`] joins the two into a project-wide graph of who imports and
//!   who calls what.
//! - [`rank`] scores files by how close they are in that graph to a few
//!   starting files.
//! - [`map`] draws the whole project on one screen, for the start of a run.
//! - [`walk`] lists a project's files the same way for every caller.

pub mod graph;
pub mod index;
pub mod language;
pub mod map;
pub mod rank;
pub mod references;
pub mod resolve;
pub mod symbols;
pub mod walk;

pub use graph::{CodeGraph, Confidence, Dependency, References, Site};
pub use index::{CodeIndex, Located};
pub use language::Language;
pub use map::{RepoMap, repo_map};
pub use references::UseKind;
pub use symbols::{Symbol, SymbolKind, extract_symbols};
