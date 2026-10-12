//! Deciding whether a tool call that changes something may run.

use async_trait::async_trait;

/// What the agent is about to do, in a form a person can judge.
#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    pub tool: String,
    pub summary: String,
}

#[async_trait]
pub trait Approver: Send + Sync {
    async fn approve(&self, request: &ApprovalRequest) -> bool;
}

/// Approves everything. For evaluation runs in throwaway directories and for
/// users who pass `--yes`.
pub struct AutoApprove;

#[async_trait]
impl Approver for AutoApprove {
    async fn approve(&self, _request: &ApprovalRequest) -> bool {
        true
    }
}
