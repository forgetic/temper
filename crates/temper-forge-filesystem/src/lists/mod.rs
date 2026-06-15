//! Helpers for listing, ordering, filtering, numbering, and mutating records.
//!
//! Grouped by responsibility: [`numbering`] allocates the next per-record
//! number, [`ordering`] sorts and compares records, [`filtering`] matches
//! records against queries, and [`mutations`] applies in-place state and
//! membership updates.

mod filtering;
mod mutations;
mod numbering;
mod ordering;

pub(crate) use filtering::{ci_job_matches_query, issue_matches_query, pull_request_matches_query};
pub(crate) use mutations::{
    apply_assignee_update, apply_label_update, normalize_string_set, normalize_user_set,
    update_issue_state, update_pull_request_state,
};
pub(crate) use numbering::{
    next_issue_comment_number, next_issue_number, next_pull_request_comment_number,
    next_pull_request_number, next_pull_request_review_number,
};
pub(crate) use ordering::{
    sort_ci_jobs, sort_ci_jobs_by_name, sort_comments, sort_issues, sort_issues_by_number,
    sort_labels, sort_pull_requests, sort_pull_requests_by_number, sort_repositories, sort_reviews,
};
