// SPDX-License-Identifier: MPL-2.0

//! Canonical, non-serializable deployment model shared by plan and apply.

use temper_config::{
    Config, CredentialSource, Credentials, ExposeSecret, LoadedPaths, Resolved, Secret,
};
use temper_forge::{CreateRepository, WebhookEvents, WebhookSpec};
use temper_provision::{ProvisionOptions, ProvisionPlan};
use temper_workflow::ValidatedWorkflow;

/// Loaded deployment metadata used for operator presentation.
#[derive(Debug, Clone, Default)]
pub struct DeploymentMetadata {
    pub name: Option<String>,
    pub topology: Option<String>,
    pub workflow_source: String,
    pub worker_pools: usize,
    pub agent_profiles: usize,
}

/// Authentication required by a Forge provisioning adapter.
#[derive(Debug, Clone)]
pub struct ForgeAuthentication {
    pub base_url: String,
    pub admin_user: Option<String>,
    pub admin_password: Option<Secret>,
    pub admin_token: Option<Secret>,
}

/// Desired webhook data. The HMAC payload stays secret until an adapter call is
/// constructed.
#[derive(Debug, Clone)]
pub struct DesiredWebhook {
    pub url: String,
    pub secret: Secret,
    pub events: WebhookEvents,
}

impl DesiredWebhook {
    pub(crate) fn expose_for_provisioning(&self) -> WebhookSpec {
        WebhookSpec {
            url: self.url.clone(),
            secret: self.secret.expose_secret().to_string(),
            events: self.events.clone(),
        }
    }
}

/// Desired state for one repository.
///
/// `plan` contains all non-secret workflow-derived roles, labels, and repository
/// policy. The common role password is removed from that plan and held as a
/// [`Secret`]; a webhook is likewise attached only at the provisioning boundary.
#[derive(Debug, Clone)]
pub struct DesiredRepository {
    pub plan: ProvisionPlan,
    identity_password: Secret,
}

impl DesiredRepository {
    /// Constructs the one canonical no-seed repository shape.
    pub fn from_workflow(
        workflow: &ValidatedWorkflow,
        owner: &str,
        name: &str,
        existing_repo: bool,
    ) -> Result<Self, String> {
        let repository = CreateRepository {
            owner: owner.to_string(),
            name: name.to_string(),
            default_branch: "main".to_string(),
            description: None,
        };
        let config = temper_reference_delivery::runner_config_for(workflow, repository);
        let default_branch = config.repository.default_branch.clone();
        let options = ProvisionOptions {
            existing_repo,
            repository_auto_init: false,
            roles: config.role_bindings.clone(),
            automation_login: temper_provision::BOT_USER.to_string(),
            password: temper_forge::config::FORGEJO_ROLE_PASSWORD.to_string(),
            token_scopes: role_token_scopes(),
            labels: Vec::new(),
            seed_commits: Vec::new(),
            webhook: None,
            intake: None,
        };
        let mut plan = ProvisionPlan::from_workflow(
            workflow,
            temper_forge::RepositoryPath::new(owner, name),
            default_branch,
            temper_forge::AccessScope::default(),
            options,
        )
        .map_err(|error| error.to_string())?;
        let identity_password = Secret::from(std::mem::take(&mut plan.password));
        debug_assert!(!plan.repository_auto_init);
        debug_assert!(plan.seed_commits.is_empty());
        Ok(Self {
            plan,
            identity_password,
        })
    }

    /// Materializes the backend provisioning call. This is the only point at
    /// which desired secret values leave their `Secret` wrappers.
    pub(crate) fn expose_for_provisioning(
        &self,
        webhook: Option<&DesiredWebhook>,
    ) -> ProvisionPlan {
        let mut plan = self.plan.clone();
        plan.password = self.identity_password.expose_secret().to_string();
        plan.webhook = webhook.map(DesiredWebhook::expose_for_provisioning);
        plan
    }
}

/// One loaded and validated deployment used by both `temper plan` and
/// `temper apply`.
///
/// This type intentionally has no `Serialize` implementation. Parsed
/// credentials are retained only for audited merge/write behavior; operational
/// secrets are represented by [`Secret`] in the desired/authentication model.
#[derive(Debug, Clone)]
pub struct DeploymentBundle {
    pub config: Config,
    pub credentials: Credentials,
    pub resolved: Resolved,
    pub loaded: LoadedPaths,
    pub credential_source: Option<CredentialSource>,
    pub metadata: DeploymentMetadata,
    pub workflow: ValidatedWorkflow,
    pub forge: ForgeAuthentication,
    pub webhook: Option<DesiredWebhook>,
    pub repositories: Vec<DesiredRepository>,
    pub admin_key: Option<String>,
}

impl DeploymentBundle {
    /// Constructs the complete backend call request for all desired repos.
    pub(crate) fn expose_provision_plans(&self) -> Vec<ProvisionPlan> {
        self.repositories
            .iter()
            .map(|repository| repository.expose_for_provisioning(self.webhook.as_ref()))
            .collect()
    }
}

fn role_token_scopes() -> Vec<temper_forge::TokenScope> {
    vec![
        temper_forge::TokenScope::WriteRepository,
        temper_forge::TokenScope::WriteIssue,
        temper_forge::TokenScope::WriteUser,
        temper_forge::TokenScope::ReadOrg,
    ]
}
