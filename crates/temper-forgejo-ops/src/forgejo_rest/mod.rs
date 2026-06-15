//! Small Forgejo REST helpers used by production provisioning and PR prep.
//!
//! Secrets are only sent in headers/basic auth. Errors include a status and a
//! short response-body snippet, never the authorization value.
//!
//! Each helper is a single `<io-event-request>` from the caller's point of
//! view: it performs one HTTP exchange on the engine's pooled client and
//! returns the parsed outcome. All protocol interpretation (status mapping,
//! conflict tolerance, response shapes) is pure code over the returned data.
//!
//! The operations are grouped by resource domain:
//! - [`client`] holds the shared transport, error, and response plumbing.
//! - [`orgs`] covers organizations, teams, and users.
//! - [`repos`] covers repository lifecycle, content, webhooks, and access.
//! - [`issues`] covers issues and pull requests.

mod client;
mod issues;
mod orgs;
mod repos;

pub use client::{Client, RestError, Result, http_client};
pub use issues::{create_issue_comment, list_issue_comment_bodies, list_pull_request_files};
pub use orgs::{
    ROLE_PASSWORD, add_team_member, create_user, ensure_org, mint_user_token, owners_team_id,
};
pub use repos::{
    add_repo_collaborator, commit_file, create_branch, enable_actions, ensure_repo,
    ensure_repo_webhook, require_repo,
};
