//! Small Forgejo REST helpers used by production provisioning and PR prep.
//!
//! Secrets are only sent in headers/basic auth. Errors include a status and a
//! short response-body snippet, never the authorization value.

use base64::Engine;
use reqwest::Client;
use serde_json::{json, Value};

/// The shared password assigned to demo role users.
pub const ROLE_PASSWORD: &str = "R0le-Phase2-e2e!";

/// Token scopes role workers need for the reference-delivery demo.
const TOKEN_SCOPES: &[&str] = &[
    "write:repository",
    "write:issue",
    "write:user",
    "read:organization",
];

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

pub fn http_client() -> Result<Client> {
    Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|err| RestError::Http(err.to_string()))
}

pub async fn ensure_org(client: &Client, base: &str, token: &str, owner: &str) -> Result<()> {
    let resp = client
        .post(format!("{base}/api/v1/orgs"))
        .header("Authorization", format!("token {token}"))
        .json(&json!({ "username": owner }))
        .send()
        .await
        .map_err(http_err)?;
    accept_or_conflict(resp, "create org").await
}

pub async fn owners_team_id(client: &Client, base: &str, token: &str, owner: &str) -> Result<i64> {
    let resp = client
        .get(format!("{base}/api/v1/orgs/{owner}/teams"))
        .header("Authorization", format!("token {token}"))
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
        .post(format!("{base}/api/v1/admin/users"))
        .header("Authorization", format!("token {token}"))
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

pub async fn add_team_member(
    client: &Client,
    base: &str,
    token: &str,
    team_id: i64,
    login: &str,
) -> Result<()> {
    let resp = client
        .put(format!("{base}/api/v1/teams/{team_id}/members/{login}"))
        .header("Authorization", format!("token {token}"))
        .send()
        .await
        .map_err(http_err)?;
    accept_or_conflict(resp, "add team member").await
}

pub async fn mint_user_token(client: &Client, base: &str, login: &str) -> Result<String> {
    let resp = client
        .post(format!("{base}/api/v1/users/{login}/tokens"))
        .basic_auth(login, Some(ROLE_PASSWORD))
        .json(&json!({
            "name": format!("harness-{login}"),
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
        .post(format!("{base}/api/v1/orgs/{owner}/repos"))
        .header("Authorization", format!("token {token}"))
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
        .post(format!(
            "{base}/api/v1/repos/{owner}/{name}/contents/{path}"
        ))
        .header("Authorization", format!("token {token}"))
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
        .get(format!("{base}/api/v1/repos/{owner}/{name}/hooks"))
        .header("Authorization", format!("token {token}"))
        .send()
        .await
        .map_err(http_err)?;
    let hooks: Value = json_ok(existing, "list repo hooks").await?;
    let already_registered = hooks
        .as_array()
        .is_some_and(|hooks| hooks.iter().any(|hook| hook_config_url(hook) == Some(url)));
    if already_registered {
        return Ok(());
    }

    let resp = client
        .post(format!("{base}/api/v1/repos/{owner}/{name}/hooks"))
        .header("Authorization", format!("token {token}"))
        .json(&json!({
            "type": "gitea",
            "active": true,
            "events": [
                "push",
                "issues",
                "issue_comment",
                "pull_request",
                "pull_request_review_approved",
                "pull_request_review_rejected",
                "pull_request_review_comment",
            ],
            "config": {
                "url": url,
                "content_type": "json",
                "secret": secret,
            },
        }))
        .send()
        .await
        .map_err(http_err)?;
    json_ok(resp, "create repo webhook").await.map(|_| ())
}

fn hook_config_url(hook: &Value) -> Option<&str> {
    hook.pointer("/config/url")
        .and_then(Value::as_str)
        .or_else(|| hook["url"].as_str())
}

pub async fn enable_actions(
    client: &Client,
    base: &str,
    token: &str,
    owner: &str,
    name: &str,
) -> Result<()> {
    let resp = client
        .patch(format!("{base}/api/v1/repos/{owner}/{name}"))
        .header("Authorization", format!("token {token}"))
        .json(&json!({ "has_actions": true }))
        .send()
        .await
        .map_err(http_err)?;
    json_ok(resp, "enable actions").await.map(|_| ())
}

fn http_err(err: reqwest::Error) -> RestError {
    RestError::Http(err.to_string())
}

async fn json_ok(resp: reqwest::Response, what: &str) -> Result<Value> {
    let status = resp.status();
    if status.is_success() {
        resp.json::<Value>().await.map_err(|err| RestError::Shape {
            what: what.into(),
            detail: err.to_string(),
        })
    } else {
        Err(api_error(resp, what, status).await)
    }
}

async fn accept_or_conflict(resp: reqwest::Response, what: &str) -> Result<()> {
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    let code = status.as_u16();
    let body = resp.text().await.unwrap_or_default();
    let lower = body.to_lowercase();
    let benign = (code == 409 || code == 422)
        && (lower.contains("exist") || lower.contains("already") || lower.contains("member"));
    if benign {
        Ok(())
    } else {
        Err(RestError::Api {
            what: what.into(),
            status: code,
            body: snippet(&body),
        })
    }
}

async fn api_error(resp: reqwest::Response, what: &str, status: reqwest::StatusCode) -> RestError {
    let body = resp.text().await.unwrap_or_default();
    RestError::Api {
        what: what.into(),
        status: status.as_u16(),
        body: snippet(&body),
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
}
