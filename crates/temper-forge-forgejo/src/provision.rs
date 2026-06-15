//! Provisioning capabilities: [`ForgeContent`] and [`ForgeAdmin`] on
//! [`ForgejoForge`].
//!
//! These implementations adapt the portable provisioning traits to Forgejo's
//! REST API. They are the absorbed counterpart of the free functions that used
//! to live in `temper-forgejo-ops::forgejo_rest::{orgs,repos}`: the exact HTTP
//! exchanges, the idempotent accept-or-conflict tolerance, the base64 content
//! encoding, the token scopes, and the full Gitea webhook event list are
//! preserved verbatim. The only change is that they now run on the backend's own
//! [`HttpClient`](crate::HttpClient) seam (so offline contract tests can drive
//! them through the recording mock) and surface failures as
//! [`ForgeError`](temper_forge::ForgeError) through the trait.
//!
//! ## Authentication / the admin token
//!
//! Every method except [`mint_token`](ForgeAdmin::mint_token) authenticates with
//! the backend's configured token (`Authorization: token <token>`). For
//! privileged provisioning that token must therefore be a Forgejo **admin**
//! token: construct the backend with [`ForgejoForge::new`] over a
//! [`ForgejoConfig`](crate::ForgejoConfig) whose `token` is the admin token. No
//! separate admin-token field is added — the backend already carries exactly one
//! token, and provisioning simply uses an admin-scoped one. `mint_token` is the
//! sole exception: Forgejo's token endpoint requires the *target user's* own
//! credentials, so it authenticates with HTTP Basic auth using the user's login
//! and the shared role password.

use crate::ids::{format_repository_id, parse_repository_id};
use crate::{EngineHttpClient, ForgejoForge, HttpClient, HttpMethod, HttpRequest, HttpResponse};
use base64::Engine;
use serde_json::{Value, json};
use temper_forge::{
    AccessGrant, AccessScope, CommitFile, CreateBranch, CreateRepository, ForgeAdmin, ForgeContent,
    ForgeError, ForgeResult, NewUser, RepoPermission, RepositoryId, RepositoryPath, TokenScope,
    WebhookEvents, WebhookSpec,
};

/// The shared password assigned to demo role users.
///
/// Kept Forgejo-side (it is a Forgejo-specific provisioning detail used by the
/// `temper-forgejo-provision` adapter): both [`ForgeAdmin::ensure_user`]'s
/// default and [`ForgeAdmin::mint_token`]'s Basic-auth fall back to it.
pub const ROLE_PASSWORD: &str = "R0le-Phase2-e2e!";

/// Exchanges a Forgejo **admin** user's login + password for an `all`-scoped
/// REST API token, authenticating with HTTP Basic auth.
///
/// `temper init` collects the admin user/password interactively but the
/// provisioner needs a *token* (the backend authenticates every privileged call
/// with `Authorization: token <token>`). Forgejo's token endpoint authenticates
/// as the *target user* over Basic auth, so this mints the admin's own token the
/// same way [`ForgeAdmin::mint_token`] mints a role user's — but with the
/// caller-supplied password (not the shared [`ROLE_PASSWORD`]) and the broad
/// `all` scope an admin needs for provisioning.
///
/// Token names are unique per process + nanosecond so re-running `temper init`
/// against the same instance does not collide on a duplicate-token conflict.
pub async fn admin_token_via_basic_auth(
    base_url: &str,
    user: &str,
    password: &str,
) -> ForgeResult<String> {
    let client = EngineHttpClient::new(base_url.to_string());
    mint_token_via_basic_auth(&client, user, password, &["all"]).await
}

/// Mints a token for `login` over HTTP Basic auth (`login:password`), returning
/// the raw `sha1`. Shared by [`admin_token_via_basic_auth`] and
/// [`ForgeAdmin::mint_token`] so the exact request shape stays in one place.
async fn mint_token_via_basic_auth<C: HttpClient>(
    client: &C,
    login: &str,
    password: &str,
    scopes: &[&str],
) -> ForgeResult<String> {
    // Forgejo token names are user-unique; a unique, non-secret name keeps a
    // repeated mint from failing with a duplicate-token conflict.
    let token_name = format!(
        "temper-{login}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    );
    let credentials =
        base64::engine::general_purpose::STANDARD.encode(format!("{login}:{password}"));
    let request = HttpRequest {
        method: HttpMethod::Post,
        path: format!("/api/v1/users/{login}/tokens"),
        query: Vec::new(),
        headers: vec![
            ("Authorization".to_string(), format!("Basic {credentials}")),
            ("Content-Type".to_string(), "application/json".to_string()),
            ("Accept".to_string(), "application/json".to_string()),
        ],
        body: Some(
            json!({
                "name": token_name,
                "scopes": scopes,
            })
            .to_string(),
        ),
    };
    let response = client
        .execute(request)
        .await
        .map_err(crate::error::map_transport_error)?;
    if !response.is_success() {
        return Err(crate::error::map_status_error("mint user token", &response));
    }
    let body: Value = serde_json::from_str(&response.body).map_err(|err| {
        ForgeError::Backend(format!("mint user token: failed to decode response: {err}"))
    })?;
    body["sha1"]
        .as_str()
        .map(str::to_string)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| {
            ForgeError::Backend("mint user token: no non-empty sha1 in token response".into())
        })
}

/// Token scopes role workers need for the reference-delivery demo, in the same
/// order the previous `temper-forgejo-ops` helper emitted them.
const DEFAULT_TOKEN_SCOPES: &[&str] = &[
    "write:repository",
    "write:issue",
    "write:user",
    "read:organization",
];

/// The repo collaborator permission token sent for each [`RepoPermission`].
fn permission_token(permission: RepoPermission) -> &'static str {
    match permission {
        RepoPermission::Read => "read",
        RepoPermission::Write => "write",
        RepoPermission::Admin => "admin",
    }
}

/// Maps a [`TokenScope`] onto its native Forgejo scope string.
fn token_scope_str(scope: TokenScope) -> &'static str {
    match scope {
        TokenScope::ReadOrg => "read:organization",
        TokenScope::WriteRepository => "write:repository",
        TokenScope::WriteIssue => "write:issue",
        TokenScope::WriteUser => "write:user",
    }
}

impl<C: HttpClient> ForgejoForge<C> {
    /// Treats a 2xx as success and a benign already-exists/already-member
    /// conflict (`409`/`422` whose body mentions exist/already/member) as success
    /// too; every other status maps to a [`ForgeError`].
    ///
    /// Mirrors the previous `forgejo_rest::client::accept_or_conflict` so
    /// re-running provisioning against partially-provisioned state converges.
    fn accept_or_conflict(context: &str, response: &HttpResponse) -> ForgeResult<()> {
        if response.is_success() {
            return Ok(());
        }
        let lower = response.body.to_lowercase();
        let benign = (response.status == 409 || response.status == 422)
            && (lower.contains("exist") || lower.contains("already") || lower.contains("member"));
        if benign {
            Ok(())
        } else {
            Err(crate::error::map_status_error(context, response))
        }
    }

    /// Sends a JSON `POST`/`PUT`/`PATCH` (or bodyless request) authenticated with
    /// the configured token, returning the raw response without status mapping.
    async fn provision_send(
        &self,
        method: HttpMethod,
        path: impl AsRef<str>,
        body: Option<Value>,
    ) -> ForgeResult<HttpResponse> {
        self.send(
            method,
            path,
            Vec::new(),
            body.map(|value| value.to_string()),
        )
        .await
    }
}

#[async_trait::async_trait]
impl<C: HttpClient> ForgeContent for ForgejoForge<C> {
    async fn ensure_repository(&self, input: CreateRepository) -> ForgeResult<RepositoryId> {
        let response = self
            .provision_send(
                HttpMethod::Post,
                format!("/orgs/{}/repos", input.owner),
                Some(json!({
                    "name": input.name,
                    "default_branch": input.default_branch,
                    "auto_init": true,
                    "private": false,
                })),
            )
            .await?;
        Self::accept_or_conflict("create repo", &response)?;
        Ok(format_repository_id(&crate::ids::RepoCoord::new(
            input.owner,
            input.name,
        )))
    }

    async fn require_repository(&self, path: &RepositoryPath) -> ForgeResult<RepositoryId> {
        let response = self
            .provision_send(
                HttpMethod::Get,
                format!("/repos/{}/{}", path.owner, path.name),
                None,
            )
            .await?;
        if response.is_success() {
            return Ok(format_repository_id(&crate::ids::RepoCoord::new(
                path.owner.clone(),
                path.name.clone(),
            )));
        }
        if response.status == 404 {
            return Err(ForgeError::NotFound(format!(
                "repository {}/{} does not exist; require_repository requires a \
                 pre-existing repo and never creates one",
                path.owner, path.name
            )));
        }
        Err(crate::error::map_status_error(
            "require existing repo",
            &response,
        ))
    }

    async fn commit_file(&self, repo: &RepositoryId, input: CommitFile) -> ForgeResult<()> {
        let coord = parse_repository_id(repo)?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(&input.contents);
        let response = self
            .provision_send(
                HttpMethod::Post,
                format!(
                    "/repos/{}/{}/contents/{}",
                    coord.owner, coord.name, input.path
                ),
                Some(json!({
                    "content": encoded,
                    "message": input.message,
                    "branch": input.branch,
                })),
            )
            .await?;
        Self::accept_or_conflict("commit file", &response)
    }

    async fn create_branch(&self, repo: &RepositoryId, input: CreateBranch) -> ForgeResult<()> {
        let coord = parse_repository_id(repo)?;
        let response = self
            .provision_send(
                HttpMethod::Post,
                format!("/repos/{}/{}/branches", coord.owner, coord.name),
                Some(json!({
                    "new_branch_name": input.new_branch,
                    "old_branch_name": input.from_branch,
                })),
            )
            .await?;
        Self::accept_or_conflict("create branch", &response)
    }
}

#[async_trait::async_trait]
impl<C: HttpClient> ForgeAdmin for ForgejoForge<C> {
    async fn ensure_owner(&self, owner: &str) -> ForgeResult<()> {
        let response = self
            .provision_send(
                HttpMethod::Post,
                "/orgs",
                Some(json!({ "username": owner })),
            )
            .await?;
        Self::accept_or_conflict("create org", &response)
    }

    async fn ensure_user(&self, input: NewUser) -> ForgeResult<()> {
        let response = self
            .provision_send(
                HttpMethod::Post,
                "/admin/users",
                Some(json!({
                    "username": input.login,
                    "email": input.email,
                    "password": input.password,
                    "must_change_password": false,
                })),
            )
            .await?;
        Self::accept_or_conflict("create user", &response)
    }

    async fn mint_token(&self, login: &str, scopes: &[TokenScope]) -> ForgeResult<String> {
        // An empty requested scope set falls back to the role-worker default set
        // (the only set the previous `forgejo_rest` helper ever sent).
        let scope_strs: Vec<&str> = if scopes.is_empty() {
            DEFAULT_TOKEN_SCOPES.to_vec()
        } else {
            scopes.iter().copied().map(token_scope_str).collect()
        };
        // Forgejo's token endpoint authenticates as the *target user*, not the
        // admin: HTTP Basic auth with the user's login and the shared role
        // password. This bypasses the token-auth `build_request` path.
        mint_token_via_basic_auth(self.http_client(), login, ROLE_PASSWORD, &scope_strs).await
    }

    async fn grant_access(&self, grant: AccessGrant) -> ForgeResult<()> {
        let coord = parse_repository_id(&grant.repo)?;
        match grant.scope {
            AccessScope::OrgOwners => {
                let team_id = self.owners_team_id(&coord.owner).await?;
                let response = self
                    .provision_send(
                        HttpMethod::Put,
                        format!("/teams/{team_id}/members/{}", grant.login),
                        None,
                    )
                    .await?;
                Self::accept_or_conflict("add team member", &response)
            }
            AccessScope::RepoCollaborator => {
                let response = self
                    .provision_send(
                        HttpMethod::Put,
                        format!(
                            "/repos/{}/{}/collaborators/{}",
                            coord.owner, coord.name, grant.login
                        ),
                        Some(json!({ "permission": permission_token(grant.permission) })),
                    )
                    .await?;
                Self::accept_or_conflict("add repo collaborator", &response)
            }
        }
    }

    async fn ensure_webhook(&self, repo: &RepositoryId, input: WebhookSpec) -> ForgeResult<()> {
        let coord = parse_repository_id(repo)?;
        let hooks_path = format!("/repos/{}/{}/hooks", coord.owner, coord.name);

        let existing = self
            .provision_send(HttpMethod::Get, &hooks_path, None)
            .await?;
        if !existing.is_success() {
            return Err(crate::error::map_status_error("list repo hooks", &existing));
        }
        let hooks: Value = serde_json::from_str(&existing.body).map_err(|err| {
            ForgeError::Backend(format!("list repo hooks: failed to decode response: {err}"))
        })?;
        let already_registered = hooks.as_array().is_some_and(|hooks| {
            hooks
                .iter()
                .any(|hook| hook_config_url(hook) == Some(input.url.as_str()))
        });
        if already_registered {
            return Ok(());
        }

        let events = webhook_events(&input.events);
        let response = self
            .provision_send(
                HttpMethod::Post,
                &hooks_path,
                Some(json!({
                    "type": "gitea",
                    "active": true,
                    "events": events,
                    "config": {
                        "url": input.url,
                        "content_type": "json",
                        "secret": input.secret,
                    },
                })),
            )
            .await?;
        if response.is_success() {
            Ok(())
        } else {
            Err(crate::error::map_status_error(
                "create repo webhook",
                &response,
            ))
        }
    }

    async fn enable_ci(&self, repo: &RepositoryId) -> ForgeResult<()> {
        let coord = parse_repository_id(repo)?;
        let response = self
            .provision_send(
                HttpMethod::Patch,
                format!("/repos/{}/{}", coord.owner, coord.name),
                Some(json!({ "has_actions": true })),
            )
            .await?;
        if response.is_success() {
            Ok(())
        } else {
            Err(crate::error::map_status_error("enable actions", &response))
        }
    }
}

impl<C: HttpClient> ForgejoForge<C> {
    /// Resolves the `Owners` team id of an organization
    /// (`GET /orgs/{owner}/teams`), used by [`AccessScope::OrgOwners`] grants.
    async fn owners_team_id(&self, owner: &str) -> ForgeResult<i64> {
        let response = self
            .provision_send(HttpMethod::Get, format!("/orgs/{owner}/teams"), None)
            .await?;
        if !response.is_success() {
            return Err(crate::error::map_status_error("list org teams", &response));
        }
        let teams: Value = serde_json::from_str(&response.body).map_err(|err| {
            ForgeError::Backend(format!("list org teams: failed to decode response: {err}"))
        })?;
        teams
            .as_array()
            .and_then(|teams| {
                teams
                    .iter()
                    .find(|team| team["name"].as_str() == Some("Owners"))
            })
            .and_then(|team| team["id"].as_i64())
            .ok_or_else(|| {
                ForgeError::Backend("owners team: no Owners team in org teams response".to_string())
            })
    }
}

/// Extracts a webhook's configured delivery URL, tolerating both the nested
/// `config.url` shape and the flat `url` field.
fn hook_config_url(hook: &Value) -> Option<&str> {
    hook.pointer("/config/url")
        .and_then(Value::as_str)
        .or_else(|| hook["url"].as_str())
}

/// The Forgejo event identifiers a [`WebhookEvents`] selection maps to.
///
/// [`WebhookEvents::All`] emits the full hard-coded Gitea event list the
/// previous `forgejo_rest::repos::ensure_repo_webhook` sent; [`WebhookEvents::Only`]
/// emits exactly the named identifiers.
fn webhook_events(events: &WebhookEvents) -> Vec<String> {
    match events {
        WebhookEvents::All => [
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
        ]
        .iter()
        .map(|event| (*event).to_string())
        .collect(),
        WebhookEvents::Only(events) => events.clone(),
    }
}
