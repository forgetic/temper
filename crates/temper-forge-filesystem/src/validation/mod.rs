//! Validation split by source of the data being checked.
//!
//! [`requests`] validates caller-supplied inputs (and optimistic-concurrency
//! preconditions) before a mutation. [`stored`] re-checks the invariants of
//! records loaded from disk, guarding against corrupted or externally edited
//! store contents.

mod requests;
mod stored;

pub(crate) use requests::{
    check_expected_version, validate_create_repository, validate_upsert_label,
};
pub(crate) use stored::{
    validate_stored_ci_jobs, validate_stored_issue_comments, validate_stored_issues,
    validate_stored_labels, validate_stored_pull_request_comments,
    validate_stored_pull_request_reviews, validate_stored_pull_requests,
};
