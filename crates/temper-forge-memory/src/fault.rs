//! A small, deterministic fault-injection hook for the in-memory backend.
//!
//! The hook lets a test force a chosen operation to fail with
//! [`ForgeError::Backend`](temper_forge::ForgeError::Backend) *before* it
//! touches in-memory state, modelling a backend that is momentarily unreachable
//! or whose stored data could not be read. Faults are one-shot and queued
//! per-operation: arming a fault adds one message; the next call to that
//! operation pops and returns it, then proceeds normally on later calls.
//!
//! Only the operations the workflow runtime exercises are fault-aware, mirroring
//! the set used by the crash-injection test wrapper. Every other [`Forge`]
//! method ignores the hook.
//!
//! [`Forge`]: temper_forge::Forge

use std::collections::HashMap;
use std::collections::VecDeque;
use temper_forge::{ForgeError, ForgeResult};

/// The fault-aware in-memory backend operations.
///
/// This deliberately covers the same mutating and load operations the workflow
/// runtime drives, so robustness and error-path tests can force a typed backend
/// failure at a precise step without corrupting any on-disk fixture.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FaultOp {
    /// `list_issues`.
    ListIssues,
    /// `create_issue`.
    CreateIssue,
    /// `get_issue_by_number`.
    GetIssueByNumber,
    /// `update_issue`.
    UpdateIssue,
    /// `list_pull_requests`.
    ListPullRequests,
    /// `create_pull_request`.
    CreatePullRequest,
    /// `get_pull_request_by_number`.
    GetPullRequestByNumber,
    /// `update_pull_request`.
    UpdatePullRequest,
    /// `merge_pull_request`.
    MergePullRequest,
}

/// Queued one-shot faults, keyed by operation.
#[derive(Debug, Default)]
pub(crate) struct FaultStore {
    queued: HashMap<FaultOp, VecDeque<String>>,
}

impl FaultStore {
    /// Arms a one-shot fault: the next call to `op` fails with `message`.
    pub(crate) fn arm(&mut self, op: FaultOp, message: String) {
        self.queued.entry(op).or_default().push_back(message);
    }

    /// Clears every armed fault.
    pub(crate) fn clear(&mut self) {
        self.queued.clear();
    }

    /// Consumes one armed fault for `op`, returning an error when present.
    pub(crate) fn take(&mut self, op: FaultOp) -> ForgeResult<()> {
        match self.queued.get_mut(&op).and_then(VecDeque::pop_front) {
            Some(message) => Err(ForgeError::Backend(message)),
            None => Ok(()),
        }
    }
}
