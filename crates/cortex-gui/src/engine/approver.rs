//! Asking the person before the agent changes anything: at the window,
//! or on a paired phone, whichever answers first.

use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use cortex_core::{ApprovalRequest, Approver};
use tokio::sync::oneshot;

pub use cortex_remote::wire::Decision;

use super::{Outbox, Update};
use crate::remote::{self, Reply};

/// A question shown in the conversation until it is answered.
pub struct PendingApproval {
    pub request: ApprovalRequest,

    /// The number a phone knows the question by. Zero when no phone can
    /// be asked.
    pub id: u64,

    reply: Reply,
}

impl PendingApproval {
    pub fn answer(self, decision: Decision) {
        self.reply.answer(decision);
    }

    /// Answered elsewhere, or its run is over.
    pub fn answered(&self) -> bool {
        self.reply.answered()
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

/// Takes the question back from the phones when it is answered, and
/// also when its run is abandoned while it waits.
struct Asked(u64);

impl Drop for Asked {
    fn drop(&mut self) {
        if let Some(bridge) = remote::bridge() {
            bridge.settle(self.0);
        }
    }
}

#[async_trait]
impl Approver for WindowApprover {
    async fn approve(&self, request: &ApprovalRequest) -> bool {
        if self.approve_all.load(Ordering::Relaxed) {
            return true;
        }

        let (sender, answer) = oneshot::channel();
        let reply = Reply::new(sender);

        let id = remote::bridge()
            .map(|bridge| bridge.ask(self.outbox.chat(), &reply))
            .unwrap_or(0);

        let _asked = Asked(id);

        self.outbox.send(Update::ApprovalNeeded(PendingApproval {
            request: request.clone(),
            id,
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
