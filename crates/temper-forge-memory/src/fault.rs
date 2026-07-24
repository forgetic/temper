//! A small, deterministic fault-injection hook for the in-memory backend.
//!
//! The hook lets a test force a chosen operation to fail with
//! failure before it touches in-memory state, modelling either a transient
//! backend outage or an optimistic-concurrency race. Faults are one-shot and
//! queued per operation: arming a fault adds one error; the next call to that
//! operation pops and returns it, then proceeds normally on later calls.
//!
//! Only the operations the workflow runtime exercises are fault-aware, mirroring
//! the set used by the crash-injection test wrapper. Every other [`Forge`]
//! method ignores the hook.
//!
//! [`Forge`]: temper_forge_model::Forge

use std::collections::HashMap;
use std::collections::VecDeque;
use temper_forge_model::{ForgeError, ForgeResult};

/// The fault-aware in-memory backend operations.
///
/// This deliberately covers the same mutating and load operations the workflow
/// runtime drives, so robustness and error-path tests can force a typed backend
/// failure at a precise step without corrupting any on-disk fixture.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FaultOp {
    /// `get_repository_by_path`.
    GetRepositoryByPath,
    /// `list_issues`.
    ListIssues,
    /// `create_issue`.
    CreateIssue,
    /// `get_issue_by_number`.
    GetIssueByNumber,
    /// `update_issue`.
    UpdateIssue,
    /// `list_issue_comments`.
    ListIssueComments,
    /// `add_issue_comment`.
    AddIssueComment,
    /// `list_pull_requests`.
    ListPullRequests,
    /// `create_pull_request`.
    CreatePullRequest,
    /// `get_pull_request_by_number`.
    GetPullRequestByNumber,
    /// `update_pull_request`.
    UpdatePullRequest,
    /// `list_pull_request_comments`.
    ListPullRequestComments,
    /// `add_pull_request_comment`.
    AddPullRequestComment,
    /// `retry_ci_attempt` (surfaced as typed uncertainty).
    RetryCiAttempt,
    /// `merge_pull_request`.
    MergePullRequest,
}

/// Queued one-shot faults, keyed by operation.
#[derive(Debug, Default)]
pub(crate) struct FaultStore {
    queued: HashMap<FaultOp, VecDeque<ForgeError>>,
}

impl FaultStore {
    /// Arms a one-shot backend fault.
    pub(crate) fn arm(&mut self, op: FaultOp, message: String) {
        self.queued
            .entry(op)
            .or_default()
            .push_back(ForgeError::Backend(message));
    }

    /// Arms a one-shot optimistic-concurrency conflict.
    pub(crate) fn arm_conflict(&mut self, op: FaultOp, message: String) {
        self.queued
            .entry(op)
            .or_default()
            .push_back(ForgeError::Conflict(message));
    }

    /// Clears every armed fault.
    pub(crate) fn clear(&mut self) {
        self.queued.clear();
    }

    /// Consumes one armed fault for `op`, returning it when present.
    pub(crate) fn take(&mut self, op: FaultOp) -> ForgeResult<()> {
        match self.queued.get_mut(&op).and_then(VecDeque::pop_front) {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}
