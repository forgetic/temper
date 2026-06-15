//! Pure conversions from GitHub DTOs into portable `temper_forge_model` models.
//!
//! These functions contain no HTTP or state; they translate the lenient DTOs in
//! [`crate::types`] into the domain types in [`temper_forge_model`]. Keeping mapping
//! pure makes the request/response plumbing in [`crate::pulls`] and
//! [`crate::items`] thin and lets the conversions be unit-tested directly.
//!
//! Determinism: labels, assignees, and requested reviewers are sorted and
//! deduplicated so two reads of the same artifact produce identical vectors,
//! matching the reference backends' contract.
//!
//! The conversions are grouped by domain: [`artifacts`] (users, repositories,
//! labels, comments), [`issues`], [`pulls`] (pull requests, branches, merge),
//! and [`reviews`].

mod artifacts;
mod issues;
mod pulls;
mod reviews;

pub(crate) use artifacts::{map_comment, map_label, map_repository, map_user};
pub(crate) use issues::map_issue;
pub(crate) use pulls::{map_pull_request, merge_method_token};
pub(crate) use reviews::{map_review, review_event_token};

use temper_forge_model::UserId;

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
