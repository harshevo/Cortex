//! Messages from the person that arrive while a run is in progress.
//!
//! Whoever owns the agent puts them in an [`Inbox`]; the agent loop takes
//! them out between steps and shows them to the model with the step's
//! tool results. If one arrives as the model is finishing, the run goes
//! on to answer it instead of ending. Nothing is interrupted: a message
//! is read at the next point where the model would look anyway.

use std::sync::Mutex;

#[derive(Debug, Default)]
pub struct Inbox {
    waiting: Mutex<Vec<String>>,
}

impl Inbox {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, message: impl Into<String>) {
        self.lock().push(message.into());
    }

    /// Everything that has arrived, oldest first, leaving the inbox empty.
    pub fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.lock())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<String>> {
        self.waiting
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// A message as the model is shown it, marked so that it is not mistaken
/// for tool output.
pub fn as_shown(message: &str) -> String {
    format!(
        "[A message from the user, sent while you were working. Take it into account from \
         here on.]\n\n{message}"
    )
}
