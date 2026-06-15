// SPDX-License-Identifier: MPL-2.0

//! Step 4 of `temper init`: provision the forge — the one step that touches a
//! network and a runtime.
//!
//! It is hidden behind the [`Provisioner`] trait so [`run_init`](crate::run_init)
//! can be exercised offline: production uses [`ForgejoProvisioner`] (mints an
//! admin REST token from the Q4 user/password, builds the Forgejo plan, runs the
//! backend-agnostic orchestration); tests pass a stub that returns a canned
//! [`Provisioned`].

use std::path::PathBuf;

use temper_provision::Provisioned;

/// The non-secret + secret inputs the provisioning step needs, distilled from
/// the collected answers + the resolved artifact paths.
#[derive(Debug, Clone)]
pub struct ProvisionRequest {
    /// Forge base URL.
    pub base_url: String,
    /// Admin login (Q4).
    pub admin_user: String,
    /// Admin password (Q4). **Secret.**
    pub admin_password: String,
    /// Repository owner.
    pub owner: String,
    /// Repository name.
    pub name: String,
    /// Webhook URL the daemon will serve, registered on the repo.
    pub webhook_url: String,
    /// Path to the freshly written webhook secret the adapter reads back.
    pub webhook_secret_file: PathBuf,
    /// Provision onto a pre-existing repo (`--existing-repo`).
    pub existing_repo: bool,
}

/// The result of a provisioning run: the minted-token identities plus the admin
/// REST token (which `temper init` stores in `credentials.toml`).
pub struct ProvisionOutcome {
    /// Provisioned role + bot identities and repository.
    pub provisioned: Provisioned,
    /// The admin REST token minted from the Q4 password. **Secret.**
    pub admin_token: String,
}

/// The injectable live-forge step. Implementors run the actual provisioning;
/// the unit test provides a stub.
pub trait Provisioner {
    /// Provisions the forge for `request`, returning the minted identities and
    /// the admin token. The `String` error is shown to the operator verbatim.
    fn provision(&mut self, request: &ProvisionRequest) -> Result<ProvisionOutcome, String>;
}

/// The production [`Provisioner`]: mints an admin token over Basic auth, then
/// runs the Forgejo provisioning adapter on a fresh engine runtime.
///
/// No intake issue is seeded (`intake_seed = None`): `temper init` stands up the
/// deployment; filing the first issue is left to the operator.
pub struct ForgejoProvisioner;

/// Token scopes role workers need for the reference-delivery demo (the same set
/// the dissolved Forgejo provisioning adapter emitted).
const ROLE_TOKEN_SCOPES: &[temper_forge_model::TokenScope] = &[
    temper_forge_model::TokenScope::WriteRepository,
    temper_forge_model::TokenScope::WriteIssue,
    temper_forge_model::TokenScope::WriteUser,
    temper_forge_model::TokenScope::ReadOrg,
];

impl Provisioner for ForgejoProvisioner {
    fn provision(&mut self, request: &ProvisionRequest) -> Result<ProvisionOutcome, String> {
        let runtime = temper_engine_io::build_runtime()?;
        let request = request.clone();
        temper_engine_io::runtime::block_on_runtime_with(&runtime, move |_cx, _handle| async move {
            // Exchange the admin user+password for an admin REST token.
            let admin_token = temper_forge_forgejo::admin_token_via_basic_auth(
                &request.base_url,
                &request.admin_user,
                &request.admin_password,
            )
            .await
            .map_err(|error| format!("mint admin token: {error}"))?;

            let workflow = temper_reference_delivery::basic_delivery_workflow();
            let config = temper_reference_delivery::runner_config_for(
                &workflow,
                temper_reference_delivery::repo_input(),
            );
            let default_branch = config.repository.default_branch.clone();

            // Read the freshly written webhook secret the daemon will serve
            // and fold it into the plan so the orchestration registers the
            // hook idempotently after the repo exists.
            let secret = std::fs::read_to_string(&request.webhook_secret_file)
                .map_err(|error| {
                    format!(
                        "read webhook secret {}: {error}",
                        request.webhook_secret_file.display()
                    )
                })?
                .trim()
                .to_string();
            let webhook = temper_forge_model::WebhookSpec {
                url: request.webhook_url.clone(),
                secret,
                events: temper_forge_model::WebhookEvents::All,
            };

            let plan_options = temper_provision::ProvisionOptions {
                existing_repo: request.existing_repo,
                roles: config.role_bindings.clone(),
                automation_login: temper_provision::BOT_USER.to_string(),
                password: temper_forge_forgejo::ROLE_PASSWORD.to_string(),
                token_scopes: ROLE_TOKEN_SCOPES.to_vec(),
                labels: Vec::new(),
                seed_commits: temper_reference_delivery::ci_seed_commits(&default_branch),
                webhook: Some(webhook),
                // No intake issue seeded by `temper init`.
                intake: None,
            };
            let plan = temper_provision::ProvisionPlan::from_workflow(
                &workflow,
                temper_forge_model::RepositoryPath::new(&request.owner, &request.name),
                default_branch,
                temper_forge_model::AccessScope::default(),
                plan_options,
            )
            .map_err(|error| error.to_string())?;

            let forge_config =
                temper_forge_forgejo::ForgejoConfig::new(&request.base_url, &admin_token)
                    .with_default_repo(&request.owner, &request.name);
            let forge = temper_forge_forgejo::ForgejoForge::new(forge_config);

            let provisioned = temper_provision::provision(&plan, &forge, &forge, &forge)
                .await
                .map_err(|error| error.to_string())?;
            Ok(ProvisionOutcome {
                provisioned,
                admin_token,
            })
        })
    }
}
