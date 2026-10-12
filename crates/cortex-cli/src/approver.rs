//! Asking the person at the terminal before the agent changes anything.

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use cortex_core::{ApprovalRequest, Approver};

/// Prompts for each mutating action. Answering `a` approves everything for
/// the rest of the session.
#[derive(Default)]
pub struct InteractiveApprover {
    approve_all: AtomicBool,
}

#[async_trait]
impl Approver for InteractiveApprover {
    async fn approve(&self, request: &ApprovalRequest) -> bool {
        if self.approve_all.load(Ordering::Relaxed) {
            return true;
        }

        let question = format!("  allow: {}?  [y]es / [n]o / [a]lways  ", request.summary);

        let answer = tokio::task::spawn_blocking(move || ask(&question))
            .await
            .unwrap_or_default();

        match answer.as_str() {
            "a" | "always" => {
                self.approve_all.store(true, Ordering::Relaxed);

                true
            }

            "y" | "yes" => true,

            // Anything else, including end of input, is a refusal.
            _ => false,
        }
    }
}

fn ask(question: &str) -> String {
    print!("{question}");

    let _ = io::stdout().flush();

    let mut line = String::new();

    if io::stdin().read_line(&mut line).is_err() {
        return String::new();
    }

    line.trim().to_lowercase()
}
