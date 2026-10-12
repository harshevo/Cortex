//! Forwarding the model's text to the window while it is being written.

use std::time::Duration;

use cortex_core::{ProviderError, StreamObserver};

use super::{Outbox, Update};

pub struct WindowStream {
    outbox: Outbox,
}

impl WindowStream {
    pub fn new(outbox: Outbox) -> Self {
        Self { outbox }
    }
}

impl StreamObserver for WindowStream {
    fn started(&self) {
        self.outbox.send(Update::WritingStarted);
    }

    fn text(&self, piece: &str) {
        self.outbox.send(Update::Writing(piece.to_string()));
    }

    fn thinking(&self, piece: &str) {
        self.outbox.send(Update::Thinking(piece.to_string()));
    }

    fn calling(&self, tool: &str) {
        self.outbox.send(Update::Calling(tool.to_string()));
    }

    fn retrying(&self, error: &ProviderError, wait: Duration) {
        let reason = match error {
            ProviderError::Http { status: 429, .. } => "Rate limited by the provider".to_string(),
            ProviderError::Http { status, .. } => format!("The provider answered {status}"),
            ProviderError::Network(_) => "The connection failed".to_string(),
            _ => "The request failed".to_string(),
        };

        self.outbox.send(Update::Retrying { reason, wait });
    }
}
