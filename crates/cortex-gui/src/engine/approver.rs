use super::{Outbox, Update};
use async_trait::async_trait;
use cortex_core::{ApprovalRequest, Approver};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::oneshot;

#[derive(Clone, Copy)]
pub enum Decision {
    Allow,
    AlwaysAllow,
    Deny,
}

pub struct PendingApproval {
    pub request: ApprovalRequest,
    reply: oneshot::Sender<Decision>,
}
impl PendingApproval {
    pub fn answered(&self) -> bool {
        self.reply.is_closed()
    }
    pub fn answer(self, decision: Decision) {
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

    /// Approves everything without asking, except what cannot be undone
    /// from a checkpoint or reaches beyond the project.
    pub fn trusting(outbox: Outbox) -> Self {
        Self {
            outbox,
            approve_all: AtomicBool::new(true),
        }
    }
}
#[async_trait]
impl Approver for WindowApprover {
    async fn approve(&self, request: &ApprovalRequest) -> bool {
        // Approving everything in advance does not cover a destructive
        // call: that is asked about each time.
        let destructive = cortex_core::destructive(request).is_some();

        if self.approve_all.load(Ordering::Relaxed) && !destructive {
            return true;
        }
        let (reply, answer) = oneshot::channel();
        self.outbox.send(Update::ApprovalNeeded(PendingApproval {
            request: request.clone(),
            reply,
        }));
        match answer.await.unwrap_or(Decision::Deny) {
            Decision::Allow => true,
            Decision::AlwaysAllow => {
                if !destructive {
                    self.approve_all.store(true, Ordering::Relaxed);
                }
                true
            }
            Decision::Deny => false,
        }
    }
}
