//! Provisioning + identity for the ignored Forgejo end-to-end fixture.
//!
//! This is the real-backend analogue of the filesystem `provision` step in
//! `crate::worker_bin::run`: it makes a *running* [`ForgejoServer`] hold the
//! org, the initialized repository, the per-role users + tokens, the workflow's
//! labels, and the CI workflow file(s) that Phase 4's scenarios need. Where the
//! filesystem backend gets identity for free via `as_user`, Forgejo identity is
//! the **token** — so this module creates a real user and mints a real access
//! token per workflow role (findings-phase-0 §3).
//!
//! Everything here is test-fixture state and lives entirely outside
//! `temper-forge` / `RunnerConfig`: the role map is keyed by [`RoleId`] but is
//! not a Forge-trait concept (constraints in the phase prompt). The known facts
//! it applies (non-reserved admin login, basic-auth token minting, Owners-team
//! membership for org write, `auto_init` repo) are from the Phase 0/0b spikes;
//! this module builds, it does not rediscover.
//!
//! Async because the label upsert goes through the async [`ForgejoForge`]
//! backend (the same code production uses), so the provisioning entry point is
//! awaited under a Tokio reactor. The admin **CLI** bootstrap is a blocking
//! one-shot subprocess (`ForgejoServer::run_cli`), matching the runner fixture.
//!
//! Secrets discipline: tokens and passwords flow through this module but are
//! **never** logged. Errors carry HTTP status + a body snippet, never the
//! `Authorization` value.

use super::provision_rest as rest;
use super::{ForgejoServer, ServerError};
use crate::{runner_config, workflow};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use temper_forge_forgejo::{ForgejoConfig, ForgejoForge};
use temper_forge_model::RepositoryId;
use temper_runner::RoleBinding;
use temper_workflow::RoleId;

/// A non-reserved admin login. `admin` itself is reserved by Forgejo
/// (`CreateUser: name is reserved`), so the spike used `e2eadmin`.
pub(super) const ADMIN_USER: &str = "e2eadmin";
const ADMIN_EMAIL: &str = "e2eadmin@example.invalid";
/// A fixed, known admin password. This is throwaway, single-process test infra
/// (the server is killed on drop), not a credential that ever reaches anything
/// real; it is never logged.
const ADMIN_PASSWORD: &str = "Adm1n-Phase2-e2e!";

/// The shared, known password every provisioned role user gets. It is reused as
/// the Phase 3b web-UI CI-read credential (findings-phase-0c), so it is returned
/// in the role map. Throwaway, never logged. `pub(super)` so the REST helpers in
/// `provision_rest` can authenticate with it.
pub(super) const ROLE_PASSWORD: &str = "R0le-Phase2-e2e!";

/// Token scopes that worked in the spike (findings-phase-0 §3). `all` also
/// works; this narrower set documents what the workers actually need.
pub(super) const TOKEN_SCOPES: &[&str] = &[
    "write:repository",
    "write:issue",
    "write:user",
    "read:organization",
];

/// The path the CI workflow is committed to. `runs-on: host` (Phase 1b runner).
pub(super) const WORKFLOW_PATH: &str = ".forgejo/workflows/ci.yml";

/// A neutral grey label color. Forgejo 7.0.12 requires a non-empty color on
/// label create/update; the workflow declares none, so every label gets this.
const LABEL_COLOR: &str = "#ededed";

/// The commit-message marker the CI workflow gates on. A head whose latest
/// commit message contains it passes CI; one without it fails.
///
/// The gate is keyed on the message of `GITHUB_SHA`, not a checked-out file,
/// because the host-mode runner has no `actions/checkout` available offline — a
/// working directory `test -f` would always fail. Reading the commit by SHA also
/// avoids depending on Forgejo's push-event `head_commit` payload ordering when
/// several quick pushes target the same branch.
pub const CI_PASS_MARKER: &str = "[ci-pass]";

/// The CI workflow committed to the provisioned repo.
///
/// **Fail→pass mechanism** (for `ci_fails_then_passes`): the single `build` job
/// passes only when `GITHUB_SHA`'s commit message contains [`CI_PASS_MARKER`].
/// The first PR head's commits do not carry it, so the job fails; the engineer's
/// *fix commit* carries the marker, so the re-run on the new head SHA passes.
/// Because a CI run is keyed by SHA (findings-phase-0b), the fail and the pass
/// live on two different head SHAs — exactly the two verdicts the scenario
/// asserts. The marker is read through Forgejo's commit API, so no checkout (and
/// thus no network for `actions/checkout`) is needed; the runner stays offline.
pub const CI_WORKFLOW: &str = r#"name: ci
on: [push]
jobs:
  build:
    runs-on: host
    steps:
      - name: gate on commit message marker
        run: |
          python3 - <<'PY'
          import json
          import os
          import sys
          import urllib.request

          api = os.environ["GITHUB_API_URL"]
          repo = os.environ["GITHUB_REPOSITORY"]
          sha = os.environ["GITHUB_SHA"]
          token = os.environ["GITHUB_TOKEN"]

          req = urllib.request.Request(
              f"{api}/repos/{repo}/git/commits/{sha}",
              headers={
                  "Authorization": f"token {token}",
                  "Accept": "application/json",
              },
          )
          with urllib.request.urlopen(req, timeout=15) as resp:
              data = json.load(resp)

          msg = data.get("message") or data.get("commit", {}).get("message", "")
          first_line = msg.splitlines()[0] if msg else ""
          print(f"commit {sha}: {first_line}")

          if "[ci-pass]" not in msg:
              print("marker absent; failing")
              sys.exit(1)

          print("marker present; passing")
          PY
"#;

/// Identity for one workflow role on the real Forgejo backend.
///
/// `user`/`token` are what a role worker needs to build a `ForgejoForge` handle
/// whose `current_user` resolves to this role; `password` is the web-UI CI-read
/// credential reused by Phase 3b. None of these are ever logged.
#[derive(Clone, Deserialize, Serialize)]
pub struct RoleIdentity {
    /// Forgejo login (matches the `RunnerConfig` role binding's user handle).
    pub user: String,
    /// Email assigned at creation (derived from the login).
    pub email: String,
    /// Personal access token minted via the user's own basic-auth.
    pub token: String,
    /// The user's known password (web-UI CI-read credential).
    pub password: String,
}

impl std::fmt::Debug for RoleIdentity {
    /// Redacts the token and password so a `{:?}` can never leak them.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RoleIdentity")
            .field("user", &self.user)
            .field("email", &self.email)
            .field("token", &"<redacted>")
            .field("password", &"<redacted>")
            .finish()
    }
}

/// Shared admin/org/role identity state that can provision many repositories.
#[derive(Clone, Deserialize, Serialize)]
pub struct ProvisionedRoles {
    /// Admin access token (scope `all`), for further admin REST if needed.
    pub admin_token: String,
    /// The owner org all role users can write to.
    pub owner: String,
    /// Per-role identity, keyed by workflow role.
    pub roles: BTreeMap<RoleId, RoleIdentity>,
}

impl std::fmt::Debug for ProvisionedRoles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProvisionedRoles")
            .field("admin_token", &"<redacted>")
            .field("owner", &self.owner)
            .field("roles", &self.roles)
            .finish()
    }
}

impl ProvisionedRoles {
    /// The identity for `role`, if provisioned.
    pub fn role(&self, role: &RoleId) -> Option<&RoleIdentity> {
        self.roles.get(role)
    }

    /// Materializes this shared identity set as a single-repository fixture.
    pub fn for_repository(&self, name: impl Into<String>, repository: RepositoryId) -> Provisioned {
        Provisioned {
            admin_token: self.admin_token.clone(),
            owner: self.owner.clone(),
            name: name.into(),
            repository,
            roles: self.roles.clone(),
        }
    }
}

/// The full result of provisioning one repository for the e2e scenarios.
#[derive(Clone, Deserialize, Serialize)]
pub struct Provisioned {
    /// Admin access token (scope `all`), for further admin REST if needed.
    pub admin_token: String,
    /// The owner/name of the provisioned org repository.
    pub owner: String,
    /// The repository name.
    pub name: String,
    /// The backend-resolved repository identifier (resolves by path).
    pub repository: RepositoryId,
    /// Per-role identity, keyed by workflow role.
    pub roles: BTreeMap<RoleId, RoleIdentity>,
}

impl std::fmt::Debug for Provisioned {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Provisioned")
            .field("admin_token", &"<redacted>")
            .field("owner", &self.owner)
            .field("name", &self.name)
            .field("repository", &self.repository)
            .field("roles", &self.roles)
            .finish()
    }
}

impl Provisioned {
    /// The identity for `role`, if provisioned.
    pub fn role(&self, role: &RoleId) -> Option<&RoleIdentity> {
        self.roles.get(role)
    }
}

/// Errors raised while provisioning a Forgejo for the e2e fixture.
#[derive(Debug)]
pub enum ProvisionError {
    /// A `forgejo` admin CLI subcommand failed.
    Cli(ServerError),
    /// An HTTP request could not be sent/received.
    Http(String),
    /// A provisioning REST call returned a non-success status.
    Api {
        /// Short label of the call that failed (no secrets).
        what: String,
        /// HTTP status code.
        status: u16,
        /// Response body snippet (provider error text; never our auth header).
        body: String,
    },
    /// A response was missing an expected field.
    Shape { what: String, detail: String },
    /// The async Forge backend rejected an operation (e.g. label upsert).
    Forge(temper_forge_model::ForgeError),
    /// The reusable Forgejo state-cache fixture failed.
    Fixture(ServerError),
}

impl std::fmt::Display for ProvisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProvisionError::Cli(err) => write!(f, "forgejo admin CLI failed: {err}"),
            ProvisionError::Http(why) => write!(f, "provisioning HTTP error: {why}"),
            ProvisionError::Api { what, status, body } => {
                write!(f, "provisioning call '{what}' failed ({status}): {body}")
            }
            ProvisionError::Shape { what, detail } => {
                write!(f, "provisioning response '{what}' malformed: {detail}")
            }
            ProvisionError::Forge(err) => write!(f, "forge operation failed: {err}"),
            ProvisionError::Fixture(err) => write!(f, "forgejo fixture failed: {err}"),
        }
    }
}

impl std::error::Error for ProvisionError {}

impl From<temper_forge_model::ForgeError> for ProvisionError {
    fn from(err: temper_forge_model::ForgeError) -> Self {
        ProvisionError::Forge(err)
    }
}

/// Crate-internal result alias; shared with the `provision_rest` helpers.
pub(super) type Result<T> = std::result::Result<T, ProvisionError>;

/// Provisions a running Forgejo for the reference-delivery e2e scenarios.
///
/// Given a booted [`ForgejoServer`], this performs the full sequence and returns
/// everything the test needs:
///
/// 1. **Admin bootstrap** — create a non-reserved admin (CLI) + mint an
///    `all`-scoped admin token (CLI).
/// 2. **Org** — `POST /admin/orgs`-equivalent via the admin user
///    (`POST /api/v1/orgs`), idempotent.
/// 3. **Per-role identity** — for each `runner_config()` role binding, create a
///    user with [`ROLE_PASSWORD`], add it to the org Owners team (org write),
///    and mint a token via the user's own **basic-auth**.
/// 4. **Repository** — an `auto_init` repo under the org so `main` exists.
/// 5. **Labels** — upsert every label the compiled workflow declares, through
///    the async [`ForgejoForge`] backend (mirrors `upsert_labels`).
/// 6. **CI** — `PATCH has_actions:true`, then commit [`CI_WORKFLOW`] to
///    [`WORKFLOW_PATH`] so the push schedules an Actions run.
///
/// The owner/name come from `runner_config().repository`; nothing is hardcoded
/// elsewhere. Idempotent where Forgejo allows it (re-creating an org/user/label
/// is tolerated), so a retried provision does not wedge.
pub async fn provision(cx: &temper_engine_io::Cx, server: &ForgejoServer) -> Result<Provisioned> {
    let config = runner_config();
    let admin_token = bootstrap_admin(server)?;
    let repos = super::provision_cache::provision_repositories(
        cx,
        server.base_url(),
        &admin_token,
        std::slice::from_ref(&config.repository.name),
    )
    .await?;
    repos
        .provisioned(&config.repository.name)
        .ok_or_else(|| ProvisionError::Shape {
            what: "provisioned repository".into(),
            detail: format!(
                "{} missing from provisioned repository map",
                config.repository.name
            ),
        })
}

/// Provisions the org, per-role identity, repository, labels, and CI workflow
/// against an **already-running** Forgejo, given an existing admin token.
///
/// This is the server-agnostic REST/Forge portion of [`provision`]: it does not
/// touch [`ForgejoServer`] at all, so an operator binary can drive it against a
/// real instance (the admin token comes from the operator's own
/// `forgejo admin user generate-access-token`, not the throwaway CLI bootstrap).
/// `provision(&server)` is the throwaway-server wrapper that bootstraps an admin
/// then calls this; both share one code path so behaviour stays identical.
///
/// `roles` is the role-binding list (from `runner_config()` or operator config),
/// so role logins stay derived from config and are never hardcoded. For multiple
/// repos in one live world, prefer [`provision_role_identities`] once and then
/// [`provision_repository`] per repo so same-name tokens are not reminted.
pub async fn provision_world(
    cx: &temper_engine_io::Cx,
    base_url: &str,
    admin_token: &str,
    owner: &str,
    name: &str,
    roles: &[RoleBinding],
    default_branch: &str,
) -> Result<Provisioned> {
    let identities = provision_role_identities(cx, base_url, admin_token, owner, roles).await?;
    provision_repository(cx, base_url, &identities, name, default_branch).await
}

/// Provisions the org and per-role Forgejo identities once for a live world.
///
/// The returned map can be reused for every repository in the same org, avoiding
/// repeated same-name token minting while still giving each role token access via
/// Owners-team membership.
pub async fn provision_role_identities(
    cx: &temper_engine_io::Cx,
    base_url: &str,
    admin_token: &str,
    owner: &str,
    roles: &[RoleBinding],
) -> Result<ProvisionedRoles> {
    let client = rest::http_client(cx.clone())?;
    rest::ensure_org(&client, base_url, admin_token, owner).await?;
    let owners_team = rest::owners_team_id(&client, base_url, admin_token, owner).await?;

    let mut role_map = BTreeMap::new();
    for binding in roles {
        debug_assert_eq!(
            binding.user.id.as_str(),
            binding.user.handle,
            "forgejo role users need id == handle so one login serves assignment and web-UI login",
        );
        let login = binding.user.handle.clone();
        let email = format!("{login}@example.invalid");
        rest::create_user(&client, base_url, admin_token, &login, &email).await?;
        rest::add_team_member(&client, base_url, admin_token, owners_team, &login).await?;
        let token = rest::mint_user_token(&client, base_url, &login).await?;
        role_map.insert(
            binding.role.clone(),
            RoleIdentity {
                user: login,
                email,
                token,
                password: ROLE_PASSWORD.to_string(),
            },
        );
    }

    Ok(ProvisionedRoles {
        admin_token: admin_token.to_string(),
        owner: owner.to_string(),
        roles: role_map,
    })
}

/// Provisions one repository using an already-created role identity map.
pub async fn provision_repository(
    cx: &temper_engine_io::Cx,
    base_url: &str,
    identities: &ProvisionedRoles,
    name: &str,
    default_branch: &str,
) -> Result<Provisioned> {
    let client = rest::http_client(cx.clone())?;
    rest::ensure_repo(
        &client,
        base_url,
        &identities.admin_token,
        &identities.owner,
        name,
        default_branch,
    )
    .await?;

    let repository =
        upsert_labels(base_url, &identities.admin_token, &identities.owner, name).await?;
    rest::enable_actions(
        &client,
        base_url,
        &identities.admin_token,
        &identities.owner,
        name,
    )
    .await?;
    rest::commit_file(
        &client,
        base_url,
        &identities.admin_token,
        &identities.owner,
        name,
        WORKFLOW_PATH,
        CI_WORKFLOW,
        "add CI workflow (runs-on: host)",
        default_branch,
    )
    .await?;

    Ok(identities.for_repository(name.to_string(), repository))
}

/// Creates a non-reserved admin user and mints an `all`-scoped token via the
/// server CLI. Two steps because `admin user create --access-token` yields a
/// **scopeless** token on 7.0.x; `generate-access-token --scopes all --raw`
/// mints a usable one (findings-phase-0b). Tolerates a pre-existing admin so a
/// re-provision against the same instance does not fail.
pub fn bootstrap_admin(server: &ForgejoServer) -> Result<String> {
    // Create may fail if the user already exists; tolerate that one case.
    if let Err(err) = server.run_cli(&[
        "admin",
        "user",
        "create",
        "--username",
        ADMIN_USER,
        "--password",
        ADMIN_PASSWORD,
        "--email",
        ADMIN_EMAIL,
        "--admin",
        "--must-change-password=false",
    ]) {
        if !already_exists(&err) {
            return Err(ProvisionError::Cli(err));
        }
    }
    let token = server
        .run_cli(&[
            "admin",
            "user",
            "generate-access-token",
            "--username",
            ADMIN_USER,
            "--scopes",
            "all",
            "--raw",
        ])
        .map_err(ProvisionError::Cli)?;
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err(ProvisionError::Shape {
            what: "admin token".into(),
            detail: "generate-access-token returned empty output".into(),
        });
    }
    Ok(token)
}

/// Whether a CLI error names an "already exists"/"reserved" duplicate, which a
/// repeated bootstrap can safely ignore.
fn already_exists(err: &ServerError) -> bool {
    let text = err.to_string().to_lowercase();
    text.contains("already exists") || text.contains("user already exists")
}

/// Upserts every label the compiled workflow declares through the async
/// [`ForgejoForge`] backend (mirrors `worker_bin::run::upsert_labels`), and
/// returns the resolved [`RepositoryId`]. Driving labels through the real
/// backend exercises the same code production uses (BUG-1 fix on `main`).
async fn upsert_labels(
    base: &str,
    admin_token: &str,
    owner: &str,
    name: &str,
) -> Result<RepositoryId> {
    use temper_forge_model::{RepositoryPath, UpsertLabel};

    let config = ForgejoConfig::new(base, admin_token).with_default_repo(owner, name);
    let forge = ForgejoForge::new(config);

    let repo = forge
        .get_repository_by_path(&RepositoryPath::new(owner, name))
        .await?
        .ok_or_else(|| ProvisionError::Shape {
            what: "repository".into(),
            detail: format!("{owner}/{name} not readable after creation"),
        })?;

    let workflow = workflow();
    let compiled = workflow.compile();
    for label in compiled.labels().labels() {
        forge
            .upsert_label(
                &repo.id,
                UpsertLabel {
                    name: label.id.to_string(),
                    // Forgejo 7.0.12 *requires* a color (`[Color]: Required`),
                    // unlike the filesystem backend where `None` is fine. The
                    // workflow declares no per-label color, so use one neutral
                    // grey for every label — the e2e cares about presence, not
                    // appearance.
                    color: Some(LABEL_COLOR.to_string()),
                    description: None,
                },
            )
            .await?;
    }
    Ok(repo.id)
}

#[cfg(test)]
#[path = "provision_tests.rs"]
mod tests;
