//! CLI argument and error types for `temper provision-forgejo`.

use std::fmt;
use std::path::PathBuf;

use crate::provision::{AccessScope, ProvisionError};

pub const ADMIN_TOKEN_ENV: &str = "TEMPER_FORGEJO_ADMIN_TOKEN";

pub const USAGE: &str = concat!(
    "temper-provision-forgejo --base-url <url> --owner <org> --name <repo> --out <path> ",
    "[--workflow <path>] ",
    "[--webhook-url <url> --webhook-secret-file <path>] ",
    "[--seed-intake yes|no] [--seed-only] [--intake-title <title>] [--intake-body-file <path>] ",
    "[--existing-repo] [--access org-owners|repo-collaborator]\n",
    "  --seed-only files just the intake issue (no org/users/repo/labels/CI/webhook work), ",
    "reusing the role tokens already written to --out; pair it with a first ",
    "--seed-intake no pass so the entry issue can be filed after the workers are up\n",
    "  --existing-repo provisions onto a repo that must already exist: it never ",
    "creates the repo and never commits CI (the marker workflow or the sentinel), ",
    "so the repo's own .forgejo/workflows/ci.yml and history stay untouched; it ",
    "still ensures labels, the webhook, and Actions enablement\n",
    "  --access selects how identities are granted access (default org-owners, ",
    "today's behavior): org-owners adds every role user and the bot to the org ",
    "Owners team; repo-collaborator instead grants each a repo-scoped write ",
    "collaborator permission and never touches the Owners team\n",
    "  the admin token comes from TEMPER_FORGEJO_ADMIN_TOKEN (required), never argv; ",
    "the workflow file comes from --workflow, defaulting to the bundled ",
    "reference-delivery workflow when unset; --out writes a credentials.toml the ",
    "daemon loads via --credentials"
);

#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum ParseOutcome {
    Run(ProvisionArgs),
    Help,
}

#[derive(Clone, Eq, PartialEq)]
pub struct ProvisionArgs {
    pub base_url: String,
    pub owner: String,
    pub name: String,
    pub out: PathBuf,
    pub admin_token: String,
    pub webhook_url: Option<String>,
    pub webhook_secret_file: Option<PathBuf>,
    pub seed_intake: bool,
    /// File only the intake issue, skipping all org/users/repo/labels/CI/webhook
    /// provisioning. The authoring role's token is recovered from `out`.
    pub seed_only: bool,
    pub intake_title: Option<String>,
    pub intake_body_file: Option<PathBuf>,
    /// Workflow document to provision against. `None` uses the bundled
    /// reference-delivery workflow, reproducing today's default behavior.
    pub workflow_file: Option<PathBuf>,
    /// Provision onto a repo that must already exist: skip repo creation and the
    /// CI/sentinel commits. Defaults to `false` (throwaway behavior).
    pub existing_repo: bool,
    /// How role users and the `bot` are granted access. Defaults to
    /// [`AccessScope::OrgOwners`] (today's behavior).
    pub access: AccessScope,
}

impl fmt::Debug for ProvisionArgs {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProvisionArgs")
            .field("base_url", &self.base_url)
            .field("owner", &self.owner)
            .field("name", &self.name)
            .field("out", &self.out)
            .field("admin_token", &"<redacted>")
            .field("webhook_url", &self.webhook_url)
            .field("webhook_secret_file", &self.webhook_secret_file)
            .field("seed_intake", &self.seed_intake)
            .field("seed_only", &self.seed_only)
            .field("intake_title", &self.intake_title)
            .field("intake_body_file", &self.intake_body_file)
            .field("workflow_file", &self.workflow_file)
            .field("existing_repo", &self.existing_repo)
            .field("access", &self.access)
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArgsError(String);

impl ArgsError {
    pub(super) fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ArgsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ArgsError {}

#[derive(Debug)]
pub enum RunError {
    Runtime(String),
    Workflow(temper_reference_delivery::WorkflowLoadError),
    Provision(ProvisionError),
}

impl fmt::Display for RunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::Runtime(why) => write!(formatter, "failed to start async runtime: {why}"),
            RunError::Workflow(error) => write!(formatter, "{error}"),
            RunError::Provision(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for RunError {}

impl From<ProvisionError> for RunError {
    fn from(error: ProvisionError) -> Self {
        Self::Provision(error)
    }
}

impl From<temper_reference_delivery::WorkflowLoadError> for RunError {
    fn from(error: temper_reference_delivery::WorkflowLoadError) -> Self {
        Self::Workflow(error)
    }
}

impl From<std::io::Error> for RunError {
    fn from(error: std::io::Error) -> Self {
        Self::Provision(ProvisionError::Io(error))
    }
}
