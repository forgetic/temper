//! Pure conversions from Forgejo DTOs into portable `temper_forge` models.
//!
//! These functions contain no HTTP or state; they translate the lenient DTOs in
//! [`crate::types`] into the domain types in [`temper_forge`]. Keeping mapping
//! pure makes the request/response plumbing in [`crate::pulls`] and
//! [`crate::items`] thin and lets the conversions be unit-tested directly.
//!
//! Determinism: labels, assignees, and requested reviewers are sorted and
//! deduplicated so two reads of the same artifact produce identical vectors,
//! matching the reference backends' contract (see ADR 0008).
//!
//! The conversions are grouped by domain: [`items`] (user/repo/label/comment/
//! issue), [`pulls`] (pull requests and their branch sides), and [`reviews`]
//! (review verdicts and the merge/review provider tokens). The shared scalar
//! helpers live here and are re-exported across the submodules.

mod items;
mod pulls;
mod reviews;

use temper_forge::UserId;

pub(crate) use items::{map_comment, map_issue, map_label, map_repository, map_user};
pub(crate) use pulls::map_pull_request;
pub(crate) use reviews::{map_review, merge_method_token, review_event_token};

/// Returns `None` for a missing or empty string, else the value unchanged.
pub(super) fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.is_empty())
}

/// Normalizes a provider state string: lowercase, spaces/dashes to underscores.
pub(super) fn normalize(value: &str) -> String {
    value.trim().to_lowercase().replace([' ', '-'], "_")
}

pub(super) fn map_logins(users: Option<Vec<crate::types::UserDto>>) -> Vec<UserId> {
    users
        .unwrap_or_default()
        .into_iter()
        .map(|user| crate::ids::format_user_id(&user.login))
        .collect()
}

pub(super) fn sorted_dedup(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values.dedup();
    values
}

pub(super) fn sorted_dedup_users(mut values: Vec<UserId>) -> Vec<UserId> {
    values.sort();
    values.dedup();
    values
}
