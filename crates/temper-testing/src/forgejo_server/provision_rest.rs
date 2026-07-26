//! Low-level Forgejo REST helpers for [`super::provision`].
//!
//! These are the raw `/api/v1` calls the provisioning orchestration composes:
//! org/user/token/repo creation, Owners-team membership, the CI workflow commit,
//! and enabling Actions, plus the small response helpers they share. They are
//! split out of `provision.rs` purely to keep each file within the source-size
//! budget; the orchestration, public types, and admin-CLI bootstrap stay there.
//!
//! Each helper is one `<io-event-request>`: a single HTTP exchange performed by
//! the engine's pooled client; all response interpretation (status mapping,
//! conflict tolerance, shapes) is pure code over the buffered response.
//!
//! Secrets discipline (same as `provision.rs`): tokens/passwords pass through
//! these calls but are never logged; errors carry a status + body snippet only.

use super::provision::{ProvisionError, ROLE_PASSWORD, Result, TOKEN_SCOPES};
use base64::Engine;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering};
use temper_engine_io::http::{HttpCall, http_call};

static NEXT_TOKEN_NAME: AtomicU64 = AtomicU64::new(0);

/// Per-request deadline, matching the previous client-wide configuration.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Engine-backed HTTP client shared by all REST provisioning helpers.
#[derive(Clone)]
pub(super) struct Client {
    /// Clock capability of the task the client was built on; request
    /// deadlines are computed against it.
    cx: skein::cx::Cx,
    inner: std::sync::Arc<skein::http::h1::http_client::HttpClient>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Client").finish_non_exhaustive()
    }
}

enum Auth<'a> {
    Token(&'a str),
    Basic(&'a str, &'a str),
}

struct RestResponse {
    status: u16,
    body: String,
}

impl RestResponse {
    fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

impl Client {
    async fn send(
        &self,
        method: &str,
        url: String,
        auth: Auth<'_>,
        body: Option<&Value>,
    ) -> Result<RestResponse> {
        let mut headers = vec![match auth {
            Auth::Token(token) => ("Authorization".to_string(), format!("token {token}")),
            Auth::Basic(user, password) => {
                let credentials =
                    base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"));
                ("Authorization".to_string(), format!("Basic {credentials}"))
            }
        }];
        if body.is_some() {
            headers.push(("Content-Type".to_string(), "application/json".to_string()));
        }
        let call = HttpCall {
            method: method.to_string(),
            url,
            headers,
            body: body
                .map(|value| value.to_string().into_bytes())
                .unwrap_or_default(),
        };

        let result = match skein::time::timeout(
            temper_engine_io::runtime::timer_now(&self.cx),
            REQUEST_TIMEOUT,
            Box::pin(http_call(&self.inner, call)),
        )
        .await
        {
            Ok(result) => result,
            Err(_elapsed) => Err(format!(
                "request timed out after {}s",
                REQUEST_TIMEOUT.as_secs()
            )),
        };

        let response = result.map_err(ProvisionError::Http)?;
        Ok(RestResponse {
            status: response.status,
            body: String::from_utf8_lossy(&response.body).into_owned(),
        })
    }
}

/// Builds the shared HTTP client used for all REST provisioning. The `cx` is
/// the calling task's clock capability; request deadlines compute against it.
pub(super) fn http_client(cx: skein::cx::Cx) -> Result<Client> {
    Ok(Client {
        cx,
        inner: temper_engine_io::http::build_http_client(),
    })
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
        .send(
            "POST",
            format!("{base}/api/v1/orgs"),
            Auth::Token(admin_token),
            Some(&json!({ "username": owner })),
        )
        .await?;
    accept_or_conflict(resp, "create org")
}

/// Finds the org's `Owners` team id (`GET /orgs/{org}/teams`).
pub(super) async fn owners_team_id(
    client: &Client,
    base: &str,
    admin_token: &str,
    owner: &str,
) -> Result<i64> {
    let resp = client
        .send(
            "GET",
            format!("{base}/api/v1/orgs/{owner}/teams"),
            Auth::Token(admin_token),
            None,
        )
        .await?;
    let teams: Value = json_ok(resp, "list org teams")?;
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
        .send(
            "POST",
            format!("{base}/api/v1/admin/users"),
            Auth::Token(admin_token),
            Some(&json!({
                "username": login,
                "email": email,
                "password": ROLE_PASSWORD,
                "must_change_password": false,
            })),
        )
        .await?;
    accept_or_conflict(resp, "create user")
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
        .send(
            "PUT",
            format!("{base}/api/v1/teams/{team_id}/members/{login}"),
            Auth::Token(admin_token),
            None,
        )
        .await?;
    accept_or_conflict(resp, "add team member")
}

/// Mints a token for `login` via the user's own **basic-auth**. The
/// admin-on-behalf shape is not part of the supported API contract. Token names
/// are unique per process so repeated single-repo provisions do not collide.
/// Returns the raw `sha1` token.
pub(super) async fn mint_user_token(client: &Client, base: &str, login: &str) -> Result<String> {
    let resp = client
        .send(
            "POST",
            format!("{base}/api/v1/users/{login}/tokens"),
            Auth::Basic(login, ROLE_PASSWORD),
            Some(&json!({
                "name": unique_token_name(login),
                "scopes": TOKEN_SCOPES,
            })),
        )
        .await?;
    let body: Value = json_ok(resp, "mint user token")?;
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
        .send(
            "POST",
            format!("{base}/api/v1/orgs/{owner}/repos"),
            Auth::Token(admin_token),
            Some(&json!({
                "name": name,
                "default_branch": default_branch,
                "auto_init": true,
                "private": false,
            })),
        )
        .await?;
    accept_or_conflict(resp, "create repo")
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
        .send(
            "POST",
            format!("{base}/api/v1/repos/{owner}/{name}/contents/{path}"),
            Auth::Token(admin_token),
            Some(&json!({
                "content": encoded,
                "message": message,
                "branch": branch,
            })),
        )
        .await?;
    accept_or_conflict(resp, "commit file")
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
        .send(
            "POST",
            format!("{base}/api/v1/repos/{owner}/{name}/branches"),
            Auth::Token(token),
            Some(&json!({
                "new_branch_name": new_branch,
                "old_branch_name": old_branch,
            })),
        )
        .await?;
    accept_or_conflict(resp, "create branch")
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
        .send(
            "PATCH",
            format!("{base}/api/v1/repos/{owner}/{name}"),
            Auth::Token(admin_token),
            Some(&json!({ "has_actions": true })),
        )
        .await?;
    json_ok(resp, "enable actions").map(|_| ())
}

fn unique_token_name(login: &str) -> String {
    let id = NEXT_TOKEN_NAME.fetch_add(1, Ordering::SeqCst);
    format!("e2e-{login}-{}-{id}", std::process::id())
}

/// Reads a JSON body, erroring on non-success status. `what` is a secret-free
/// label of the call.
fn json_ok(resp: RestResponse, what: &str) -> Result<Value> {
    if resp.is_success() {
        serde_json::from_str::<Value>(&resp.body).map_err(|err| ProvisionError::Shape {
            what: what.into(),
            detail: err.to_string(),
        })
    } else {
        Err(api_error(resp, what))
    }
}

/// Accepts a success, or a "already exists"/"already a member" conflict so the
/// step is idempotent on a re-provision. Any other status is an error.
fn accept_or_conflict(resp: RestResponse, what: &str) -> Result<()> {
    if resp.is_success() {
        return Ok(());
    }
    // 409/422 with a "exists"/"member" body means the prior provision already
    // did this; tolerate it.
    let code = resp.status;
    let lower = resp.body.to_lowercase();
    let benign = (code == 409 || code == 422)
        && (lower.contains("exist") || lower.contains("already") || lower.contains("member"));
    if benign {
        Ok(())
    } else {
        Err(ProvisionError::Api {
            what: what.into(),
            status: code,
            body: snippet(&resp.body),
        })
    }
}

fn api_error(resp: RestResponse, what: &str) -> ProvisionError {
    ProvisionError::Api {
        what: what.into(),
        status: resp.status,
        body: snippet(&resp.body),
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
