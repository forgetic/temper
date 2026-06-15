//! Provisioning types, options, errors, and the bundled CI workflow constant.

use std::collections::BTreeMap;
use std::fmt;

use temper_forge::RepositoryId;
use temper_forgejo_ops::forgejo_rest::RestError;
use temper_workflow::RoleId;

pub(crate) const WORKFLOW_PATH: &str = ".forgejo/workflows/ci.yml";
pub(crate) const LABEL_COLOR: &str = "#ededed";
/// Automation account the mechanical worker uses for workflow actions such as
/// landing approved, green PRs and reading Forgejo Actions status. It is kept
/// separate from the setup-only site admin so the admin never participates in
/// the workflow.
pub const BOT_USER: &str = "bot";
pub const DEFAULT_INTAKE_TITLE: &str = "Add a configurable greeting to the service banner";
pub const DEFAULT_INTAKE_BODY: &str = "As an operator I want the service banner to show a \
configurable greeting so I can tell environments apart at a glance.\n\n\
Acceptance: a `BANNER_GREETING` setting whose value is printed on startup, \
defaulting to the current text when unset.";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IntakeIssueSeed {
    pub title: String,
    pub body: String,
}

impl Default for IntakeIssueSeed {
    fn default() -> Self {
        Self {
            title: DEFAULT_INTAKE_TITLE.into(),
            body: DEFAULT_INTAKE_BODY.into(),
        }
    }
}

// NOTE: this MUST be a raw string. A normal string literal with `\<newline>`
// continuations strips the leading whitespace of each continued source line,
// which is exactly the YAML indentation — the committed workflow then lands
// flush-left, is invalid, and Forgejo silently detects no workflow (no CI runs
// ever fire). See agent lesson 0013.
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

/// How provisioned identities (role users + the `bot`) are granted access to
/// the target repo.
///
/// The canonical definition now lives in `temper-forge` as shared provisioning
/// vocabulary; this is a temporary re-export shim so existing
/// `crate::provision::AccessScope` paths keep working unchanged. Issue #180
/// removes the shim and switches call sites to the `temper-forge` path.
pub use temper_forge::AccessScope;

/// Options that tune [`provision_world`](super::provision_world) away from its
/// throwaway-repo defaults.
///
/// Both fields default to today's behavior, so `ProvisionOptions::default()`
/// leaves the throwaway `reference-delivery` / `basic-delivery` flows unchanged.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProvisionOptions {
    /// Provision onto a repo that must already exist: require the repo up front
    /// (erroring if absent), and skip the marker CI commit and the CI sentinel
    /// commit so the repo's own `.forgejo/workflows/ci.yml` and history are
    /// never touched. Labels, the webhook, and `enable_actions` still apply.
    pub existing_repo: bool,
    /// How role users and the `bot` are granted access to the repo.
    pub access: AccessScope,
}

/// The repo-scoped collaborator permission granted to role users and the `bot`
/// under [`AccessScope::RepoCollaborator`]. `write` lets the bot merge approved,
/// green PRs and read Actions status over the web UI (ADR-0019); `admin` is
/// intentionally avoided until a concrete need appears.
pub(crate) const REPO_COLLABORATOR_PERMISSION: &str = "write";

#[derive(Clone)]
pub struct RoleIdentity {
    pub user: String,
    pub email: String,
    pub token: String,
    pub password: String,
}

impl fmt::Debug for RoleIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RoleIdentity")
            .field("user", &self.user)
            .field("email", &self.email)
            .field("token", &"<redacted>")
            .field("password", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Debug)]
pub struct Provisioned {
    pub owner: String,
    pub name: String,
    pub repository: RepositoryId,
    pub roles: BTreeMap<RoleId, RoleIdentity>,
    /// The `bot` automation identity used by the mechanical worker.
    pub automation: RoleIdentity,
}

#[derive(Debug)]
pub enum ProvisionError {
    Rest(RestError),
    Forge(temper_forge::ForgeError),
    Shape { what: String, detail: String },
    Io(std::io::Error),
}

impl fmt::Display for ProvisionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProvisionError::Rest(err) => write!(formatter, "{err}"),
            ProvisionError::Forge(err) => write!(formatter, "forge operation failed: {err}"),
            ProvisionError::Shape { what, detail } => {
                write!(
                    formatter,
                    "provisioning response '{what}' malformed: {detail}"
                )
            }
            ProvisionError::Io(err) => write!(formatter, "writing secrets file failed: {err}"),
        }
    }
}

impl std::error::Error for ProvisionError {}

impl From<RestError> for ProvisionError {
    fn from(err: RestError) -> Self {
        Self::Rest(err)
    }
}

impl From<temper_forge::ForgeError> for ProvisionError {
    fn from(err: temper_forge::ForgeError) -> Self {
        Self::Forge(err)
    }
}

impl From<std::io::Error> for ProvisionError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

pub type Result<T> = std::result::Result<T, ProvisionError>;
