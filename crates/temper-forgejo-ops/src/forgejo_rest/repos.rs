//! Repository lifecycle operations: creation, content, branches, webhooks,
//! actions, and collaborators.

use base64::Engine;
use serde_json::{Value, json};

use super::client::{Auth, Client, RestError, Result, accept_or_conflict, api_error, json_ok};

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
