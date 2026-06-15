//! Provisioning plan, options, and result types.

use std::collections::BTreeMap;
use std::fmt;

use temper_forge::{
    AccessScope, CommitFile, RepositoryId, RepositoryPath, TokenScope, WebhookSpec,
};
use temper_runner::RoleBinding;
use temper_workflow::RoleId;

/// Automation account the mechanical worker uses for workflow actions such as
/// landing approved, green PRs and reading CI status. It is kept separate from
/// the setup-only site admin so the admin never participates in the workflow.
pub const BOT_USER: &str = "bot";

/// A single label to upsert into the target repository.
///
/// The orchestration upserts each of these via
/// [`Forge::upsert_label`](temper_forge::Forge::upsert_label).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LabelSpec {
    /// Label name to create or update.
    pub name: String,
    /// Optional label color (`#rrggbb`).
    pub color: Option<String>,
    /// Optional label description.
    pub description: Option<String>,
}

/// The entry "intake" issue to seed once the world is provisioned.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IntakeIssueSeed {
    /// Issue title.
    pub title: String,
    /// Issue body.
    pub body: String,
}

impl IntakeIssueSeed {
    /// Builds an intake seed from a title and body.
    pub fn new(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
        }
    }
}

/// Host-supplied bits a backend adapter injects into a [`ProvisionPlan`].
///
/// [`ProvisionPlan::from_workflow`] takes these as input rather than reaching
/// into any reference-delivery defaults: the adapter passes in the role
/// bindings, the seed commits (e.g. the Forgejo CI workflow + sentinel), the
/// webhook, and the intake seed it wants applied.
#[derive(Clone, Debug, Default)]
pub struct ProvisionOptions {
    /// Provision onto a repo that must already exist: require the repo up front
    /// (erroring if absent) and skip the [`seed_commits`](Self::seed_commits)
    /// so the repo's own content and history are never touched. Labels, the
    /// webhook, grants, and CI enablement still apply.
    pub existing_repo: bool,
    /// Role users to provision, each bound to a workflow role.
    pub roles: Vec<RoleBinding>,
    /// Login of the automation (bot) identity to provision alongside the roles.
    pub automation_login: String,
    /// Password assigned to every provisioned identity (role users and the
    /// automation account). Backends that mint tokens via the user's own
    /// credentials (e.g. Forgejo Basic auth) require this to match the password
    /// their token endpoint expects. **Secret** — never log it.
    pub password: String,
    /// Token scopes minted for each provisioned identity. An empty set lets the
    /// backend pick its default scopes.
    pub token_scopes: Vec<TokenScope>,
    /// Labels to upsert into the target repository.
    pub labels: Vec<LabelSpec>,
    /// Files to commit onto the default branch after the repo exists. Skipped
    /// when [`existing_repo`](Self::existing_repo) is set.
    pub seed_commits: Vec<CommitFile>,
    /// Optional webhook to register on the target repository.
    pub webhook: Option<WebhookSpec>,
    /// Optional entry intake issue to seed once provisioning completes.
    pub intake: Option<IntakeIssueSeed>,
}

/// A distilled, backend-agnostic provisioning plan.
///
/// Built from a [`ValidatedWorkflow`](temper_workflow::ValidatedWorkflow) plus
/// repository coordinates, an [`AccessScope`], and host-supplied
/// [`ProvisionOptions`] via [`ProvisionPlan::from_workflow`], then handed to
/// [`provision`](crate::provision).
#[derive(Clone, Debug)]
pub struct ProvisionPlan {
    /// Repository to provision.
    pub repo: RepositoryPath,
    /// Default branch the seed commits target and the repo is created with.
    pub default_branch: String,
    /// Role users to provision, each bound to a workflow role.
    pub roles: Vec<RoleBinding>,
    /// Login of the automation (bot) identity.
    pub automation_login: String,
    /// Password assigned to every provisioned identity. **Secret** — never log
    /// it.
    pub password: String,
    /// How role users and the automation identity are granted access.
    pub access: AccessScope,
    /// Token scopes minted for each provisioned identity (empty = backend
    /// default).
    pub token_scopes: Vec<TokenScope>,
    /// Labels to upsert into the target repository.
    pub labels: Vec<LabelSpec>,
    /// Whether the repository must already exist (`true`) or is created
    /// (`false`).
    pub existing_repo: bool,
    /// Optional webhook to register on the target repository.
    pub webhook: Option<WebhookSpec>,
    /// Optional entry intake issue to seed.
    pub intake: Option<IntakeIssueSeed>,
    /// Labels applied to the seeded intake issue, resolved from the workflow at
    /// [`from_workflow`](Self::from_workflow) time. Empty when the workflow's
    /// entry issue kind is the default (catch-all) kind.
    pub intake_labels: Vec<String>,
    /// Files to commit onto the default branch (e.g. the backend's CI workflow
    /// and CI sentinel). Skipped when [`existing_repo`](Self::existing_repo) is
    /// set.
    pub seed_commits: Vec<CommitFile>,
}

/// A provisioned identity (a role user or the automation account) and its
/// minted token.
#[derive(Clone)]
pub struct RoleIdentity {
    /// Backend login of the identity.
    pub user: String,
    /// Email address of the identity.
    pub email: String,
    /// Minted access token. **Secret** — never log this field.
    pub token: String,
    /// Account password. **Secret** — never log this field.
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

/// The result of a successful [`provision`](crate::provision) run.
#[derive(Clone, Debug)]
pub struct Provisioned {
    /// Repository owner.
    pub owner: String,
    /// Repository name.
    pub name: String,
    /// Stable identifier of the provisioned repository.
    pub repository: RepositoryId,
    /// Provisioned role identities keyed by workflow role.
    pub roles: BTreeMap<RoleId, RoleIdentity>,
    /// The automation (bot) identity used by the mechanical worker.
    pub automation: RoleIdentity,
}

/// The email convention for a provisioned login: `<login>@example.invalid`.
pub(crate) fn role_email(login: &str) -> String {
    format!("{login}@example.invalid")
}
