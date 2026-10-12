//! Asking the person at the window before the agent changes anything.

use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use cortex_core::{ApprovalRequest, Approver};
use tokio::sync::oneshot;

use super::{Outbox, Update};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,

    /// Allow this and everything after it, for the rest of the session.
    AlwaysAllow,

    Deny,
}

/// A question shown in the conversation until it is answered.
pub struct PendingApproval {
    pub request: ApprovalRequest,

    reply: oneshot::Sender<Decision>,
}

impl PendingApproval {
    pub fn answer(self, decision: Decision) {
        // The tool is gone only if its run was dropped.
        let _ = self.reply.send(decision);
    }
}

pub struct WindowApprover {
    outbox: Outbox,
    approve_all: AtomicBool,
}

impl WindowApprover {
    pub fn new(outbox: Outbox) -> Self {
        Self {
            outbox,
            approve_all: AtomicBool::new(false),
        }
    }
}

#[async_trait]
impl Approver for WindowApprover {
    async fn approve(&self, request: &ApprovalRequest) -> bool {
        if self.approve_all.load(Ordering::Relaxed) {
            return true;
        }

        let (reply, answer) = oneshot::channel();

        self.outbox.send(Update::ApprovalNeeded(PendingApproval {
            request: request.clone(),
            reply,
        }));

        // A window that closed without answering has not approved.
        match answer.await.unwrap_or(Decision::Deny) {
            Decision::Allow => true,

            Decision::AlwaysAllow => {
                self.approve_all.store(true, Ordering::Relaxed);

                true
            }

            Decision::Deny => false,
        }
    }
}
