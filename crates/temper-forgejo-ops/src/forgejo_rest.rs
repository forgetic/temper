//! Small Forgejo REST helpers used by production provisioning and PR prep.
//!
//! Secrets are only sent in headers/basic auth. Errors include a status and a
//! short response-body snippet, never the authorization value.
//!
//! Each helper is a single `<io-event-request>` from the caller's point of
//! view: it performs one HTTP exchange on the engine's pooled client and
//! returns the parsed outcome. All protocol interpretation (status mapping,
//! conflict tolerance, response shapes) is pure code over the returned data.

use base64::Engine;
use serde_json::{json, Value};
use temper_io_engine::http::{http_call, HttpCall};

/// The shared password assigned to demo role users.
pub const ROLE_PASSWORD: &str = "R0le-Phase2-e2e!";

/// Token scopes role workers need for the reference-delivery demo.
const TOKEN_SCOPES: &[&str] = &[
    "write:repository",
    "write:issue",
    "write:user",
    "read:organization",
];

/// Per-request deadline, matching the previous client-wide configuration.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Debug)]
pub enum RestError {
    Http(String),
    Api {
        what: String,
        status: u16,
        body: String,
    },
    Shape {
        what: String,
        detail: String,
    },
}

impl std::fmt::Display for RestError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RestError::Http(why) => write!(formatter, "forgejo HTTP error: {why}"),
            RestError::Api { what, status, body } => {
                write!(formatter, "forgejo call '{what}' failed ({status}): {body}")
            }
            RestError::Shape { what, detail } => {
                write!(formatter, "forgejo response '{what}' malformed: {detail}")
            }
        }
    }
}

impl std::error::Error for RestError {}

pub type Result<T> = std::result::Result<T, RestError>;

/// Engine-backed HTTP client for the low-level Forgejo helpers.
///
/// Keeps the name (and the `&Client` parameter convention) of the previous
/// reqwest-based client so call sites stay unchanged.
#[derive(Clone)]
pub struct Client {
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

/// A fully-buffered response: status plus body text.
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
            temper_io_engine::runtime::timer_now(&self.cx),
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

        let response = result.map_err(RestError::Http)?;
        Ok(RestResponse {
            status: response.status,
            body: String::from_utf8_lossy(&response.body).into_owned(),
        })
    }
}

/// Builds the REST client. The `cx` is the calling task's clock capability;
/// every request deadline is computed against it.
pub fn http_client(cx: skein::cx::Cx) -> Result<Client> {
    Ok(Client {
        cx,
        inner: temper_io_engine::http::build_http_client(),
    })
}

pub async fn ensure_org(client: &Client, base: &str, token: &str, owner: &str) -> Result<()> {
    let resp = client
        .send(
            "POST",
            format!("{base}/api/v1/orgs"),
            Auth::Token(token),
            Some(&json!({ "username": owner })),
        )
        .await?;
    accept_or_conflict(resp, "create org")
}

pub async fn owners_team_id(client: &Client, base: &str, token: &str, owner: &str) -> Result<i64> {
    let resp = client
        .send(
            "GET",
            format!("{base}/api/v1/orgs/{owner}/teams"),
            Auth::Token(token),
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
        .ok_or_else(|| RestError::Shape {
            what: "owners team".into(),
            detail: "no Owners team in org teams response".into(),
        })
}

pub async fn create_user(
    client: &Client,
    base: &str,
    token: &str,
    login: &str,
    email: &str,
) -> Result<()> {
    let resp = client
        .send(
            "POST",
            format!("{base}/api/v1/admin/users"),
            Auth::Token(token),
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

pub async fn add_team_member(
    client: &Client,
    base: &str,
    token: &str,
    team_id: i64,
    login: &str,
) -> Result<()> {
    let resp = client
        .send(
            "PUT",
            format!("{base}/api/v1/teams/{team_id}/members/{login}"),
            Auth::Token(token),
            None,
        )
        .await?;
    accept_or_conflict(resp, "add team member")
}

pub async fn mint_user_token(client: &Client, base: &str, login: &str) -> Result<String> {
    // Forgejo token names are user-unique. The reference demo may run the
    // provisioner once per configured repository, reusing the same role users;
    // give each minted token a unique, non-secret name so repeated provisioning
    // does not fail with a duplicate-token conflict.
    let token_name = format!(
        "temper-{login}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    );
    let resp = client
        .send(
            "POST",
            format!("{base}/api/v1/users/{login}/tokens"),
            Auth::Basic(login, ROLE_PASSWORD),
            Some(&json!({
                "name": token_name,
                "scopes": TOKEN_SCOPES,
            })),
        )
        .await?;
    let body: Value = json_ok(resp, "mint user token")?;
    body["sha1"]
        .as_str()
        .map(str::to_string)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| RestError::Shape {
            what: "user token".into(),
            detail: "no non-empty sha1 in token response".into(),
        })
}

pub async fn ensure_repo(
    client: &Client,
    base: &str,
    token: &str,
    owner: &str,
    name: &str,
    default_branch: &str,
) -> Result<()> {
    let resp = client
        .send(
            "POST",
            format!("{base}/api/v1/orgs/{owner}/repos"),
            Auth::Token(token),
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

#[allow(clippy::too_many_arguments)]
pub async fn commit_file(
    client: &Client,
    base: &str,
    token: &str,
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
            Auth::Token(token),
            Some(&json!({
                "content": encoded,
                "message": message,
                "branch": branch,
            })),
        )
        .await?;
    accept_or_conflict(resp, "commit file")
}

pub async fn create_branch(
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

pub async fn ensure_repo_webhook(
    client: &Client,
    base: &str,
    token: &str,
    owner: &str,
    name: &str,
    url: &str,
    secret: &str,
) -> Result<()> {
    let existing = client
        .send(
            "GET",
            format!("{base}/api/v1/repos/{owner}/{name}/hooks"),
            Auth::Token(token),
            None,
        )
        .await?;
    let hooks: Value = json_ok(existing, "list repo hooks")?;
    let already_registered = hooks
        .as_array()
        .is_some_and(|hooks| hooks.iter().any(|hook| hook_config_url(hook) == Some(url)));
    if already_registered {
        return Ok(());
    }

    let resp = client
        .send(
            "POST",
            format!("{base}/api/v1/repos/{owner}/{name}/hooks"),
            Auth::Token(token),
            Some(&json!({
                "type": "gitea",
                "active": true,
                "events": [
                    "push",
                    "status",
                    "issues",
                    "issue_comment",
                    "pull_request",
                    "pull_request_sync",
                    "pull_request_review_approved",
                    "pull_request_review_rejected",
                    "pull_request_review_comment",
                    "workflow_job",
                    "workflow_run",
                    "action_run_failure",
                    "action_run_recover",
                    "action_run_success",
                ],
                "config": {
                    "url": url,
                    "content_type": "json",
                    "secret": secret,
                },
            })),
        )
        .await?;
    json_ok(resp, "create repo webhook").map(|_| ())
}

fn hook_config_url(hook: &Value) -> Option<&str> {
    hook.pointer("/config/url")
        .and_then(Value::as_str)
        .or_else(|| hook["url"].as_str())
}

pub async fn list_pull_request_files(
    client: &Client,
    base: &str,
    token: &str,
    owner: &str,
    name: &str,
    number: u64,
) -> Result<Vec<String>> {
    let mut files = Vec::new();
    let mut page = 1;
    loop {
        let resp = client
            .send(
                "GET",
                format!(
                    "{base}/api/v1/repos/{owner}/{name}/pulls/{number}/files?page={page}&limit=50"
                ),
                Auth::Token(token),
                None,
            )
            .await?;
        let payload: Value = json_ok(resp, "list pull request files")?;
        let page_files = payload.as_array().ok_or_else(|| RestError::Shape {
            what: "pull request files".into(),
            detail: "response was not an array".into(),
        })?;
        for file in page_files {
            let filename = file["filename"]
                .as_str()
                .or_else(|| file["name"].as_str())
                .ok_or_else(|| RestError::Shape {
                    what: "pull request file".into(),
                    detail: "file entry had no filename".into(),
                })?;
            files.push(filename.to_string());
        }
        if page_files.len() < 50 {
            return Ok(files);
        }
        page += 1;
    }
}

pub async fn list_issue_comment_bodies(
    client: &Client,
    base: &str,
    token: &str,
    owner: &str,
    name: &str,
    number: u64,
) -> Result<Vec<String>> {
    let mut comments = Vec::new();
    let mut page = 1;
    loop {
        let resp = client
            .send(
                "GET",
                format!(
                    "{base}/api/v1/repos/{owner}/{name}/issues/{number}/comments?page={page}&limit=50"
                ),
                Auth::Token(token),
                None,
            )
            .await?;
        let payload: Value = json_ok(resp, "list issue comments")?;
        let page_comments = payload.as_array().ok_or_else(|| RestError::Shape {
            what: "issue comments".into(),
            detail: "response was not an array".into(),
        })?;
        for comment in page_comments {
            let body = comment["body"].as_str().ok_or_else(|| RestError::Shape {
                what: "issue comment".into(),
                detail: "comment entry had no body".into(),
            })?;
            comments.push(body.to_string());
        }
        if page_comments.len() < 50 {
            return Ok(comments);
        }
        page += 1;
    }
}

pub async fn create_issue_comment(
    client: &Client,
    base: &str,
    token: &str,
    owner: &str,
    name: &str,
    number: u64,
    body: &str,
) -> Result<()> {
    let resp = client
        .send(
            "POST",
            format!("{base}/api/v1/repos/{owner}/{name}/issues/{number}/comments"),
            Auth::Token(token),
            Some(&json!({ "body": body })),
        )
        .await?;
    json_ok(resp, "create issue comment").map(|_| ())
}

pub async fn enable_actions(
    client: &Client,
    base: &str,
    token: &str,
    owner: &str,
    name: &str,
) -> Result<()> {
    let resp = client
        .send(
            "PATCH",
            format!("{base}/api/v1/repos/{owner}/{name}"),
            Auth::Token(token),
            Some(&json!({ "has_actions": true })),
        )
        .await?;
    json_ok(resp, "enable actions").map(|_| ())
}

/// Confirms an org repository already exists, erroring clearly if it is absent.
///
/// Used by `--existing-repo` provisioning to refuse silently creating a bare
/// repo when the operator named a real target that does not exist (e.g. a typo).
/// A `404` becomes a distinct, actionable error rather than a generic API
/// failure; any other non-success status is surfaced verbatim.
pub async fn require_repo(
    client: &Client,
    base: &str,
    token: &str,
    owner: &str,
    name: &str,
) -> Result<()> {
    let resp = client
        .send(
            "GET",
            format!("{base}/api/v1/repos/{owner}/{name}"),
            Auth::Token(token),
            None,
        )
        .await?;
    if resp.is_success() {
        return Ok(());
    }
    if resp.status == 404 {
        return Err(RestError::Api {
            what: "require existing repo".into(),
            status: 404,
            body: format!(
                "repository {owner}/{name} does not exist; --existing-repo requires a \
                 pre-existing repo and never creates one"
            ),
        });
    }
    Err(api_error(resp, "require existing repo"))
}

/// Grants a repo-scoped collaborator permission on `owner/name` to `login`.
///
/// `permission` is one of `"read" | "write" | "admin"`. Routed through
/// `accept_or_conflict` so re-granting an already-collaborating user on a re-run
/// is benign. This is the repo-scoped alternative to Owners-team membership used
/// by `--access repo-collaborator`: it confers access to a single repository
/// instead of every repo in the org.
pub async fn add_repo_collaborator(
    client: &Client,
    base: &str,
    token: &str,
    owner: &str,
    name: &str,
    login: &str,
    permission: &str,
) -> Result<()> {
    let resp = client
        .send(
            "PUT",
            format!("{base}/api/v1/repos/{owner}/{name}/collaborators/{login}"),
            Auth::Token(token),
            Some(&json!({ "permission": permission })),
        )
        .await?;
    accept_or_conflict(resp, "add repo collaborator")
}

fn json_ok(resp: RestResponse, what: &str) -> Result<Value> {
    if resp.is_success() {
        serde_json::from_str::<Value>(&resp.body).map_err(|err| RestError::Shape {
            what: what.into(),
            detail: err.to_string(),
        })
    } else {
        Err(api_error(resp, what))
    }
}

fn accept_or_conflict(resp: RestResponse, what: &str) -> Result<()> {
    if resp.is_success() {
        return Ok(());
    }
    let code = resp.status;
    let lower = resp.body.to_lowercase();
    let benign = (code == 409 || code == 422)
        && (lower.contains("exist") || lower.contains("already") || lower.contains("member"));
    if benign {
        Ok(())
    } else {
        Err(RestError::Api {
            what: what.into(),
            status: code,
            body: snippet(&resp.body),
        })
    }
}

fn api_error(resp: RestResponse, what: &str) -> RestError {
    RestError::Api {
        what: what.into(),
        status: resp.status,
        body: snippet(&resp.body),
    }
}

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
    fn rest_error_display_does_not_assume_secrets() {
        let error = RestError::Api {
            what: "create repo".into(),
            status: 422,
            body: "already exists".into(),
        };
        assert_eq!(
            error.to_string(),
            "forgejo call 'create repo' failed (422): already exists"
        );
    }

    #[test]
    fn accept_or_conflict_tolerates_benign_conflicts() {
        let benign = RestResponse {
            status: 409,
            body: "user already exists".into(),
        };
        assert!(accept_or_conflict(benign, "create user").is_ok());

        let hostile = RestResponse {
            status: 500,
            body: "boom".into(),
        };
        assert!(accept_or_conflict(hostile, "create user").is_err());
    }
}
