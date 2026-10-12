//! Forwarding the agent's events to the window.

use cortex_core::{Event, TrajectorySink};

use super::{Outbox, Update};

pub struct WindowSink {
    outbox: Outbox,
}

impl WindowSink {
    pub fn new(outbox: Outbox) -> Self {
        Self { outbox }
    }
}

impl TrajectorySink for WindowSink {
    fn record(&self, event: &Event) {
        self.outbox.send(Update::Event(event.clone()));
    }
}
