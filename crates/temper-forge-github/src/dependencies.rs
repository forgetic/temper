//! Dependency-link operations for issues and pull requests.
//!
//! GitHub exposes no native issue-dependency surface over its stable REST API
//! (the closest features — task-list relationships and the sub-issues preview —
//! carry different semantics and stability guarantees). This first pass
//! therefore makes the limitation explicit instead of emulating links:
//!
//! - Reads report no dependencies ([`crate::map`] leaves the `dependencies`
//!   vector empty), which is the safe, documented behavior for scans.
//! - Mutations fail with [`ForgeError::InvalidRequest`] rather than silently
//!   claiming success, so a workflow that needs native links discovers the gap
//!   immediately.
//!
//! A later pass can adapt this module to GitHub's sub-issues API once it is
//! stable enough to rely on.

use crate::ids::{parse_issue_id, parse_pull_request_id};
use crate::{GitHubForge, HttpClient};
use temper_forge::{
    ForgeError, ForgeResult, Issue, IssueId, ItemNumber, PullRequest, PullRequestId,
};

/// Error message returned by every dependency mutation.
const UNSUPPORTED: &str =
    "github backend does not support native dependency links over the stable REST API";

impl<C: HttpClient> GitHubForge<C> {
    /// Rejects adding a dependency link: unsupported on GitHub.
    pub async fn add_issue_dependency(
        &self,
        id: &IssueId,
        _target: ItemNumber,
    ) -> ForgeResult<Issue> {
        // Validate the id shape so callers still get the right error for a
        // foreign id, then report the unsupported operation.
        parse_issue_id(id)?;
        Err(ForgeError::InvalidRequest(format!(
            "add dependency to issue {id}: {UNSUPPORTED}"
        )))
    }

    /// Rejects removing a dependency link: unsupported on GitHub.
    pub async fn remove_issue_dependency(
        &self,
        id: &IssueId,
        _target: ItemNumber,
    ) -> ForgeResult<Issue> {
        parse_issue_id(id)?;
        Err(ForgeError::InvalidRequest(format!(
            "remove dependency from issue {id}: {UNSUPPORTED}"
        )))
    }

    /// Rejects adding a dependency link: unsupported on GitHub.
    pub async fn add_pull_request_dependency(
        &self,
        id: &PullRequestId,
        _target: ItemNumber,
    ) -> ForgeResult<PullRequest> {
        parse_pull_request_id(id)?;
        Err(ForgeError::InvalidRequest(format!(
            "add dependency to pull request {id}: {UNSUPPORTED}"
        )))
    }

    /// Rejects removing a dependency link: unsupported on GitHub.
    pub async fn remove_pull_request_dependency(
        &self,
        id: &PullRequestId,
        _target: ItemNumber,
    ) -> ForgeResult<PullRequest> {
        parse_pull_request_id(id)?;
        Err(ForgeError::InvalidRequest(format!(
            "remove dependency from pull request {id}: {UNSUPPORTED}"
        )))
    }
}
