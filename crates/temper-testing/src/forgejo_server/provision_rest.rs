//! Low-level Forgejo REST helpers for [`super::provision`].
//!
//! These are the raw `/api/v1` calls the provisioning orchestration composes:
//! org/user/token/repo creation, Owners-team membership, the CI workflow commit,
//! and enabling Actions, plus the small response helpers they share. They are
//! split out of `provision.rs` purely to keep each file within the source-size
//! budget; the orchestration, public types, and admin-CLI bootstrap stay there.
//!
//! Secrets discipline (same as `provision.rs`): tokens/passwords pass through
//! these calls but are never logged; errors carry a status + body snippet only.

use super::provision::{ProvisionError, Result, ROLE_PASSWORD, TOKEN_SCOPES};
use base64::Engine;
use reqwest::Client;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TOKEN_NAME: AtomicU64 = AtomicU64::new(0);

/// Builds the shared blocking-free HTTP client used for all REST provisioning.
pub(super) fn http_client() -> Result<Client> {
    Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|err| ProvisionError::Http(err.to_string()))
}

/// Creates the org if absent. A `422`/`409` (already exists) is tolerated so the
/// step is idempotent.
pub(super) async fn ensure_org(
    client: &Client,
    base: &str,
    admin_token: &str,
    owner: &str,
) -> Result<()> {
    let resp = client
        .post(format!("{base}/api/v1/orgs"))
        .header("Authorization", format!("token {admin_token}"))
        .json(&json!({ "username": owner }))
        .send()
        .await
        .map_err(http_err)?;
    accept_or_conflict(resp, "create org").await
}

/// Finds the org's `Owners` team id (`GET /orgs/{org}/teams`).
pub(super) async fn owners_team_id(
    client: &Client,
    base: &str,
    admin_token: &str,
    owner: &str,
) -> Result<i64> {
    let resp = client
        .get(format!("{base}/api/v1/orgs/{owner}/teams"))
        .header("Authorization", format!("token {admin_token}"))
        .send()
        .await
        .map_err(http_err)?;
    let teams: Value = json_ok(resp, "list org teams").await?;
    teams
        .as_array()
        .and_then(|teams| {
            teams
                .iter()
                .find(|team| team["name"].as_str() == Some("Owners"))
        })
        .and_then(|team| team["id"].as_i64())
        .ok_or_else(|| ProvisionError::Shape {
            what: "owners team".into(),
            detail: "no Owners team in org teams response".into(),
        })
}

/// Creates a role user with the known password (`POST /admin/users`).
/// Idempotent: an existing user is tolerated.
pub(super) async fn create_user(
    client: &Client,
    base: &str,
    admin_token: &str,
    login: &str,
    email: &str,
) -> Result<()> {
    let resp = client
        .post(format!("{base}/api/v1/admin/users"))
        .header("Authorization", format!("token {admin_token}"))
        .json(&json!({
            "username": login,
            "email": email,
            "password": ROLE_PASSWORD,
            "must_change_password": false,
        }))
        .send()
        .await
        .map_err(http_err)?;
    accept_or_conflict(resp, "create user").await
}

/// Adds `login` to the Owners team (`PUT /teams/{id}/members/{user}` → 204),
/// granting org write. Idempotent.
pub(super) async fn add_team_member(
    client: &Client,
    base: &str,
    admin_token: &str,
    team_id: i64,
    login: &str,
) -> Result<()> {
    let resp = client
        .put(format!("{base}/api/v1/teams/{team_id}/members/{login}"))
        .header("Authorization", format!("token {admin_token}"))
        .send()
        .await
        .map_err(http_err)?;
    accept_or_conflict(resp, "add team member").await
}

/// Mints a token for `login` via the user's own **basic-auth** (admin-on-behalf
/// 404s on 7.0.x; findings-phase-0 §3). Token names are unique per process so
/// repeated single-repo provisions do not collide. Returns the raw `sha1` token.
pub(super) async fn mint_user_token(client: &Client, base: &str, login: &str) -> Result<String> {
    let resp = client
        .post(format!("{base}/api/v1/users/{login}/tokens"))
        .basic_auth(login, Some(ROLE_PASSWORD))
        .json(&json!({
            "name": unique_token_name(login),
            "scopes": TOKEN_SCOPES,
        }))
        .send()
        .await
        .map_err(http_err)?;
    let body: Value = json_ok(resp, "mint user token").await?;
    body["sha1"]
        .as_str()
        .map(str::to_string)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| ProvisionError::Shape {
            what: "user token".into(),
            detail: "no non-empty sha1 in token response".into(),
        })
}

/// Creates the org repo with `auto_init:true` so `main` exists for PRs.
/// Idempotent: an existing repo is tolerated.
pub(super) async fn ensure_repo(
    client: &Client,
    base: &str,
    admin_token: &str,
    owner: &str,
    name: &str,
    default_branch: &str,
) -> Result<()> {
    let resp = client
        .post(format!("{base}/api/v1/orgs/{owner}/repos"))
        .header("Authorization", format!("token {admin_token}"))
        .json(&json!({
            "name": name,
            "default_branch": default_branch,
            "auto_init": true,
            "private": false,
        }))
        .send()
        .await
        .map_err(http_err)?;
    accept_or_conflict(resp, "create repo").await
}

/// Commits a file to the repo (`POST …/contents/{path}`, base64 body).
/// Idempotent-ish: if the file already exists the `422` is tolerated.
///
/// The argument list is wide because each is a distinct, unrelated REST input
/// (no natural grouping struct); a request struct would add ceremony without
/// clarity for this single internal caller.
#[allow(clippy::too_many_arguments)]
pub(super) async fn commit_file(
    client: &Client,
    base: &str,
    admin_token: &str,
    owner: &str,
    name: &str,
    path: &str,
    contents: &str,
    message: &str,
    branch: &str,
) -> Result<()> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(contents);
    let resp = client
        .post(format!(
            "{base}/api/v1/repos/{owner}/{name}/contents/{path}"
        ))
        .header("Authorization", format!("token {admin_token}"))
        .json(&json!({
            "content": encoded,
            "message": message,
            "branch": branch,
        }))
        .send()
        .await
        .map_err(http_err)?;
    accept_or_conflict(resp, "commit file").await
}

/// Creates a branch off `old_branch` (`POST …/branches`).
/// Idempotent: a `409`/`422` "branch already exists" is tolerated, so a
/// re-attempt of the same PR-prep does not error.
pub(super) async fn create_branch(
    client: &Client,
    base: &str,
    token: &str,
    owner: &str,
    name: &str,
    new_branch: &str,
    old_branch: &str,
) -> Result<()> {
    let resp = client
        .post(format!("{base}/api/v1/repos/{owner}/{name}/branches"))
        .header("Authorization", format!("token {token}"))
        .json(&json!({
            "new_branch_name": new_branch,
            "old_branch_name": old_branch,
        }))
        .send()
        .await
        .map_err(http_err)?;
    accept_or_conflict(resp, "create branch").await
}

/// Enables Actions on the repo (`PATCH …/repos/{o}/{r} {has_actions:true}`).
pub(super) async fn enable_actions(
    client: &Client,
    base: &str,
    admin_token: &str,
    owner: &str,
    name: &str,
) -> Result<()> {
    let resp = client
        .patch(format!("{base}/api/v1/repos/{owner}/{name}"))
        .header("Authorization", format!("token {admin_token}"))
        .json(&json!({ "has_actions": true }))
        .send()
        .await
        .map_err(http_err)?;
    json_ok(resp, "enable actions").await.map(|_| ())
}

fn http_err(err: reqwest::Error) -> ProvisionError {
    ProvisionError::Http(err.to_string())
}

fn unique_token_name(login: &str) -> String {
    let id = NEXT_TOKEN_NAME.fetch_add(1, Ordering::SeqCst);
    format!("e2e-{login}-{}-{id}", std::process::id())
}

/// Reads a JSON body, erroring on non-success status. `what` is a secret-free
/// label of the call.
async fn json_ok(resp: reqwest::Response, what: &str) -> Result<Value> {
    let status = resp.status();
    if status.is_success() {
        resp.json::<Value>()
            .await
            .map_err(|err| ProvisionError::Shape {
                what: what.into(),
                detail: err.to_string(),
            })
    } else {
        Err(api_error(resp, what, status).await)
    }
}

/// Accepts a success, or a "already exists"/"already a member" conflict so the
/// step is idempotent on a re-provision. Any other status is an error.
async fn accept_or_conflict(resp: reqwest::Response, what: &str) -> Result<()> {
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    // 409/422 with a "exists"/"member" body means the prior provision already
    // did this; tolerate it. Read the body once for the decision and the error.
    let code = status.as_u16();
    let body = resp.text().await.unwrap_or_default();
    let lower = body.to_lowercase();
    let benign = (code == 409 || code == 422)
        && (lower.contains("exist") || lower.contains("already") || lower.contains("member"));
    if benign {
        Ok(())
    } else {
        Err(ProvisionError::Api {
            what: what.into(),
            status: code,
            body: snippet(&body),
        })
    }
}

async fn api_error(
    resp: reqwest::Response,
    what: &str,
    status: reqwest::StatusCode,
) -> ProvisionError {
    let body = resp.text().await.unwrap_or_default();
    ProvisionError::Api {
        what: what.into(),
        status: status.as_u16(),
        body: snippet(&body),
    }
}

/// Trims a response body to a short, log-safe snippet (char-boundary safe).
fn snippet(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.chars().count() > 300 {
        let head: String = trimmed.chars().take(300).collect();
        format!("{head}…")
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippet_truncates_long_bodies() {
        let long = "x".repeat(500);
        let out = snippet(&long);
        // 300 ASCII chars + the 3-byte ellipsis.
        assert_eq!(out.chars().count(), 301);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn snippet_passes_short_bodies_through() {
        assert_eq!(snippet("  hi  "), "hi");
    }

    #[test]
    fn token_names_are_unique_for_repeated_role_provisions() {
        let first = unique_token_name("engineer");
        let second = unique_token_name("engineer");
        assert!(first.starts_with("e2e-engineer-"));
        assert_ne!(first, second);
    }
}
