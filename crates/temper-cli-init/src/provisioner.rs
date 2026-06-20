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
/// No intake issue or repository seed content is created: `temper init` stands
/// up the deployment and forge integration, while the first issue and any
/// project files are left to explicit operator/template actions.
pub struct ForgejoProvisioner;

/// Token scopes provisioned role workers need for the embedded basic-delivery
/// workflow.
const ROLE_TOKEN_SCOPES: &[temper_forge::TokenScope] = &[
    temper_forge::TokenScope::WriteRepository,
    temper_forge::TokenScope::WriteIssue,
    temper_forge::TokenScope::WriteUser,
    temper_forge::TokenScope::ReadOrg,
];

/// Builds the repository provisioning plan used by `temper init` after the
/// webhook secret has been read. This intentionally requests no seed commits:
/// first-run initialization must not install CI workflows, sentinels, or any
/// other project content in the user's repository.
fn build_init_plan(
    request: &ProvisionRequest,
    webhook: temper_forge::WebhookSpec,
) -> Result<temper_provision::ProvisionPlan, String> {
    let workflow = temper_reference_delivery::basic_delivery_workflow();
    let config = temper_reference_delivery::runner_config_for(
        &workflow,
        temper_reference_delivery::repo_input(),
    );
    let default_branch = config.repository.default_branch.clone();
    let plan_options = temper_provision::ProvisionOptions {
        existing_repo: request.existing_repo,
        repository_auto_init: false,
        roles: config.role_bindings.clone(),
        automation_login: temper_provision::BOT_USER.to_string(),
        password: temper_forge::config::FORGEJO_ROLE_PASSWORD.to_string(),
        token_scopes: ROLE_TOKEN_SCOPES.to_vec(),
        labels: Vec::new(),
        seed_commits: Vec::new(),
        webhook: Some(webhook),
        // No intake issue seeded by `temper init`.
        intake: None,
    };
    temper_provision::ProvisionPlan::from_workflow(
        &workflow,
        temper_forge::RepositoryPath::new(&request.owner, &request.name),
        default_branch,
        temper_forge::AccessScope::default(),
        plan_options,
    )
    .map_err(|error| error.to_string())
}

impl Provisioner for ForgejoProvisioner {
    fn provision(&mut self, request: &ProvisionRequest) -> Result<ProvisionOutcome, String> {
        let runtime = temper_engine_io::build_runtime()?;
        let request = request.clone();
        temper_engine_io::runtime::block_on_runtime_with(&runtime, move |_cx, _handle| async move {
            // Exchange the admin user+password for an admin REST token.
            let admin_token = temper_forge::config::forgejo_admin_token_via_basic_auth(
                &request.base_url,
                &request.admin_user,
                &request.admin_password,
            )
            .await
            .map_err(|error| format!("mint admin token: {error}"))?;

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
            let webhook = temper_forge::WebhookSpec {
                url: request.webhook_url.clone(),
                secret,
                events: temper_forge::WebhookEvents::All,
            };

            let plan = build_init_plan(&request, webhook)?;

            let forge_config =
                temper_forge::config::ForgejoConfig::new(&request.base_url, &admin_token)
                    .with_default_repo(&request.owner, &request.name);
            let forge = temper_forge::factory::new_forgejo_provisioning(forge_config);

            let provisioned =
                temper_provision::provision(&plan, forge.as_ref(), forge.as_ref(), forge.as_ref())
                    .await
                    .map_err(|error| error.to_string())?;
            Ok(ProvisionOutcome {
                provisioned,
                admin_token,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_plan_requests_no_reference_delivery_ci_seed_commits() {
        let request = ProvisionRequest {
            base_url: "http://forge.local:3000".to_string(),
            admin_user: "root".to_string(),
            admin_password: "admin-pass".to_string(),
            owner: "acme".to_string(),
            name: "service".to_string(),
            webhook_url: "http://127.0.0.1:8314/forgejo/webhook".to_string(),
            webhook_secret_file: PathBuf::from("webhook-secret"),
            existing_repo: false,
        };
        let webhook = temper_forge::WebhookSpec {
            url: request.webhook_url.clone(),
            secret: "secret".to_string(),
            events: temper_forge::WebhookEvents::All,
        };

        let plan = build_init_plan(&request, webhook).expect("init plan builds");

        let reference_seed_paths: Vec<_> =
            temper_reference_delivery::ci_seed_commits(&plan.default_branch)
                .into_iter()
                .map(|seed| seed.path)
                .collect();
        assert!(
            reference_seed_paths
                .iter()
                .any(|path| path == ".forgejo/workflows/ci.yml"),
            "reference-delivery seed commits should include the CI workflow for this regression test"
        );
        assert!(
            reference_seed_paths
                .iter()
                .any(|path| path.starts_with(".temper-ci/")),
            "reference-delivery seed commits should include a CI sentinel for this regression test"
        );
        assert!(
            plan.seed_commits.is_empty(),
            "temper init must not request repository seed commits: {:?}",
            plan.seed_commits
        );
    }
}
