//! Human approval of `apply_ddl` deploys. The agent can only *request* a deploy; the user
//! approves or rejects it in the Schemetry app (the only caller of [`decide`]). An approval
//! is bound to the exact (connection, object, type, sha256-of-DDL) the user reviewed, lasts
//! a few minutes and is spent by the first execution — so a changed script, another
//! object or a replay all need a fresh approval.
//!
//! [`decide`]: McpApprovalService::decide

use std::sync::Arc;

use anyhow::Result;

pub use crate::repositories::mcp_apply_approval_repository::{
    ApplyRequest, ApplyStatus, McpApprovalRepository, NewApplyRequest,
};

/// How long an unanswered request stays open.
const PENDING_TTL_SECS: i64 = 60 * 60;
/// How long an approval can be used by the agent's retry.
const APPROVED_TTL_SECS: i64 = 15 * 60;
/// Expired rows are kept this long (for the GUI/debugging) before being dropped.
const RETENTION_SECS: i64 = 7 * 24 * 60 * 60;

pub struct McpApprovalService {
    repo: Arc<dyn McpApprovalRepository>,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl McpApprovalService {
    pub fn new(repo: Arc<dyn McpApprovalRepository>) -> Self {
        Self::with_clock(repo, Arc::new(|| chrono::Utc::now().timestamp()))
    }

    /// Supplies the "now" (unix seconds), so tests can move time.
    pub fn with_clock(
        repo: Arc<dyn McpApprovalRepository>,
        clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> Self {
        Self { repo, clock }
    }

    pub fn init_db(&self) -> Result<()> {
        self.repo.init_db()
    }

    /// Files (or re-finds) the request for this deploy.
    pub fn request(&self, new: &NewApplyRequest) -> Result<ApplyRequest> {
        let now = (self.clock)();
        self.repo.prune(now - RETENTION_SECS)?;
        self.repo.request(new, now, now + PENDING_TTL_SECS)
    }

    /// Spends the approval for exactly this deploy; the approval id when there was one.
    pub fn consume(&self, request: &NewApplyRequest) -> Result<Option<i64>> {
        self.repo.consume(
            request.connection_id,
            &request.object_type,
            &request.object_name,
            &request.sha256,
            (self.clock)(),
        )
    }

    /// GUI only: approve or reject a pending request. `false` if it expired or was
    /// already decided.
    pub fn decide(&self, id: i64, approve: bool) -> Result<bool> {
        let now = (self.clock)();
        self.repo.decide(id, approve, now, now + APPROVED_TTL_SECS)
    }

    pub fn list_open(&self) -> Result<Vec<ApplyRequest>> {
        self.repo.list_open((self.clock)())
    }
}
