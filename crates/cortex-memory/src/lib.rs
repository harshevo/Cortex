//! Memory for the agent, in three tiers with different lifetimes.
//!
//! | Tier | Holds | Lives for |
//! |---|---|---|
//! | Working (short-term) | what this run has read, changed, run and got wrong | one task |
//! | Episodic | one record per finished task: what worked and what failed | months |
//! | Long-term | facts about the repository, promoted from several episodes | until contradicted |
//!
//! After every task the working memory is consolidated: it is compressed
//! into an episode, and patterns seen across episodes are promoted to
//! long-term facts. Before the next task in the same repository, the most
//! similar episodes and the current facts are placed in front of the
//! instruction.
//!
//! Nothing here is learned in weights. This is the retrieval baseline that
//! the learned memory models have to beat.
//!
//! - [`working`] builds the working memory from the events of a run.
//! - [`episode`] and [`fact`] are the two stored kinds of memory.
//! - [`lesson`] is advice the model took from a run, failed or passed.
//! - [`consolidate`] promotes patterns from episodes to facts.
//! - [`store`] keeps both in SQLite.
//! - [`similarity`] ranks memories against a new task.
//! - [`recall`] chooses what to show and renders it.
//! - [`hope`] lets the learned memory model re-score what was found.
//! - [`system`] ties them together behind the core traits.

pub mod consolidate;
pub mod episode;
pub mod fact;
pub mod hope;
pub mod lesson;
pub mod recall;
pub mod similarity;
pub mod store;
pub mod system;
pub mod working;

pub use episode::Episode;
pub use fact::{Fact, FactKind};
pub use hope::HopeClient;
pub use lesson::StoredLesson;
pub use recall::{Recall, TrackRecord};
pub use similarity::{Embedder, Reranker};
pub use store::{MemoryError, Store};
pub use system::{MemoryConfig, MemorySystem};
pub use working::WorkingMemory;
