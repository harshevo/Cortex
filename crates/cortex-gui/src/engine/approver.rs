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
