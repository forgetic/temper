//! Forgejo CI marker helpers used by provisioning.
//!
//! Production role workers no longer carry a synthetic PR-prep adapter. Product
//! code branches must come from a declared-and-bound coding workspace, and PR
//! review/merge workers keep the diff guard enabled by default. This module only
//! retains the commit-message marker helper used to make the provisioned default
//! branch's CI workflow pass after setup.

use crate::forgejo_rest::{self, RestError};

const CI_SENTINEL_DIR: &str = ".harness-ci";
pub const CI_PASS_MARKER: &str = "[ci-pass]";

pub async fn commit_ci_sentinel(
    base_url: &str,
    token: &str,
    owner: &str,
    name: &str,
    branch: &str,
) -> Result<(), RestError> {
    if branch.is_empty() {
        return Err(RestError::Shape {
            what: "ci-sentinel branch".into(),
            detail: "target branch is empty".into(),
        });
    }
    let client = forgejo_rest::http_client()?;
    let safe: String = branch
        .chars()
        .map(|c| if c == '/' { '-' } else { c })
        .collect();
    let path = format!("{CI_SENTINEL_DIR}/{safe}.txt");
    let message = format!("ci pass for {branch} {CI_PASS_MARKER}");
    forgejo_rest::commit_file(
        &client,
        base_url,
        token,
        owner,
        name,
        &path,
        &format!("ci pass marker for {branch}\n"),
        &message,
        branch,
    )
    .await
}
