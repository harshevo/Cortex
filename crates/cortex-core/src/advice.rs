//! Advice given to the model between steps.
//!
//! An [`Adviser`] watches a run and may add a short note after the tool
//! results of a step, before the model decides what to do next. It only
//! ever adds text. The model stays free to ignore it, and nothing is run
//! or blocked on an adviser's say-so.

use serde_json::Value;

#[derive(Debug, Clone)]
pub struct Advice {
    /// Which component produced this, for the trajectory log.
    pub source: String,

    /// The note shown to the model.
    pub text: String,

    /// How the advice was reached. Logged, never shown to the model.
    pub detail: Value,
}

pub trait Adviser: Send + Sync {
    fn begin_run(&self, _max_steps: u32) {}
    fn prepare(&self, _step: u32, _max_steps: u32) {}
    fn inspect(&self, _query: &Value) -> Option<Value> {
        self.telemetry()
    }
    /// Bounded diagnostic data for the session log and simulator, never a prompt.
    fn telemetry(&self) -> Option<Value> {
        None
    }

    /// Called after the tools of `step` have run. `None` means there is
    /// nothing worth saying, which should be the usual answer.
    fn advise(&self, step: u32, max_steps: u32) -> Option<Advice>;
}
