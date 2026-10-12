//! Evaluation harness.
//!
//! A suite is a directory of tasks. Each task directory holds:
//!
//! - `task.toml`: the instruction given to the agent and the command that
//!   decides whether the result is correct.
//! - `repo/`: the project the agent starts from.
//! - `hidden/`: files copied in only after the agent has finished, usually
//!   the tests that score it. The agent never sees them.
//! - `solution/`: a reference fix, used only to check that the task itself
//!   is sound. The agent never sees it either.
//!
//! A long task can list `[[milestone]]` parts in `task.toml`, each with
//! its own command, so an attempt that did some of the work scores above
//! one that did none.
//!
//! Two measurements need no task to be run: [`locate`] asks whether the
//! context gathered for a task names the right files, and [`preserve`]
//! asks whether shortening a conversation changes the model's next step.

pub mod check;
pub mod compare;
pub mod locate;
pub mod preserve;
pub mod report;
pub mod runner;
pub mod task;
pub mod validate;
pub mod workdir;

pub use compare::{Comparison, Verdict};
pub use report::{SuiteReport, TaskResult, Totals};
pub use runner::{RunOptions, run_suite};
pub use task::{Suite, Task};
pub use validate::{TaskValidation, validate_suite};
