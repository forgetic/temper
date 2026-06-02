//! Deterministic identifier helpers for the in-memory backend.
//!
//! These mirror the identifier scheme of the filesystem backend so the two
//! reference backends produce the same stable ids for the same inputs. Keeping
//! the schemes aligned lets tests move between backends without re-baking
//! identifier expectations.

use temper_forge::{
    CommentId, IssueId, ItemNumber, LabelId, PullRequestId, RepositoryId, ReviewId,
};

/// Builds the deterministic repository id for a numeric counter value.
pub(crate) fn repository_id(number: u64) -> RepositoryId {
    RepositoryId::new(format!("repo-{number:016}"))
}

/// Builds the deterministic issue id for a repository-scoped number.
pub(crate) fn issue_id(repo_id: &RepositoryId, number: ItemNumber) -> IssueId {
    IssueId::new(format!("issue-{}-{:016}", repo_id.as_str(), number.get()))
}

/// Builds the deterministic pull-request id for a repository-scoped number.
pub(crate) fn pull_request_id(repo_id: &RepositoryId, number: ItemNumber) -> PullRequestId {
    PullRequestId::new(format!(
        "pull-request-{}-{:016}",
        repo_id.as_str(),
        number.get()
    ))
}

/// Builds the deterministic comment id for an issue-scoped number.
pub(crate) fn issue_comment_id(issue_id: &IssueId, number: u64) -> CommentId {
    comment_id(issue_id.as_str(), number)
}

/// Builds the deterministic comment id for a pull-request-scoped number.
pub(crate) fn pull_request_comment_id(pull_request_id: &PullRequestId, number: u64) -> CommentId {
    comment_id(pull_request_id.as_str(), number)
}

fn comment_id(target_id: &str, number: u64) -> CommentId {
    CommentId::new(format!("comment-{target_id}-{number:016}"))
}

/// Builds the deterministic review id for a pull-request-scoped number.
pub(crate) fn pull_request_review_id(pull_request_id: &PullRequestId, number: u64) -> ReviewId {
    ReviewId::new(format!("review-{pull_request_id}-{number:016}"))
}

/// Builds the deterministic label id from a repository id and label name.
pub(crate) fn label_id(repo_id: &RepositoryId, label_name: &str) -> LabelId {
    LabelId::new(format!(
        "label-{}-{}",
        repo_id.as_str(),
        hex_encode(label_name.as_bytes())
    ))
}

/// Builds the deterministic merge-commit pseudo-SHA for a logical clock tick.
pub(crate) fn merge_commit_sha(clock_tick: u64) -> String {
    format!("{clock_tick:040x}")
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX_DIGITS[(byte >> 4) as usize] as char);
        encoded.push(HEX_DIGITS[(byte & 0x0f) as usize] as char);
    }
    encoded
}
