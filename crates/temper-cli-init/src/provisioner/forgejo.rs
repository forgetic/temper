// SPDX-License-Identifier: MPL-2.0

//! Forgejo provisioning adapter.

use temper_config::{ExposeSecret, Secret};

use super::{
    ApplyPlanOutcome, ApplyPlanRequest, ApplyProvisioner, ProvisionOutcome, ProvisionRequest,
    Provisioner, build_init_plan,
};

/// Production init/apply adapter.
pub struct ForgejoProvisioner;

impl Provisioner for ForgejoProvisioner {
    fn provision(&mut self, request: &ProvisionRequest) -> Result<ProvisionOutcome, String> {
        let runtime = temper_engine_io::build_runtime()?;
        let request = request.clone();
        temper_engine_io::runtime::block_on_runtime_with(&runtime, move |_cx, _handle| async move {
            let admin_token = temper_forge::config::forgejo_admin_token_via_basic_auth(
                &request.base_url,
                &request.admin_user,
                request.admin_password.expose_secret(),
            )
            .await
            .map_err(|error| format!("mint admin token: {error}"))?;

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
            let provisioned = temper_provision::provision_with(&plan, forge.as_ref())
                .await
                .map_err(|error| error.to_string())?;
            Ok(ProvisionOutcome {
                provisioned,
                admin_token: Secret::from(admin_token),
            })
        })
    }
}

impl ApplyProvisioner for ForgejoProvisioner {
    fn provision_apply_plan(
        &mut self,
        request: &ApplyPlanRequest,
    ) -> Result<ApplyPlanOutcome, String> {
        if request.plans.is_empty() {
            return Err("deployment plan contains no repositories".to_string());
        }
        let runtime = temper_engine_io::build_runtime()?;
        let request = request.clone();
        temper_engine_io::runtime::block_on_runtime_with(&runtime, move |_cx, _handle| async move {
            let admin_token = match request.admin_token {
                Some(token) => token,
                None => {
                    let admin_user = request.admin_user.as_deref().ok_or_else(|| {
                        "mint admin token: deployment has no admin user".to_string()
                    })?;
                    let admin_password = request.admin_password.as_ref().ok_or_else(|| {
                        "mint admin token: deployment admin has no password".to_string()
                    })?;
                    let token = temper_forge::config::forgejo_admin_token_via_basic_auth(
                        &request.base_url,
                        admin_user,
                        admin_password.expose_secret(),
                    )
                    .await
                    .map_err(|error| format!("mint admin token: {error}"))?;
                    Secret::from(token)
                }
            };

            let mut provisioned = Vec::with_capacity(request.plans.len());
            for plan in request.plans {
                let owner = plan.repo.owner.clone();
                let name = plan.repo.name.clone();
                let forge_config = temper_forge::config::ForgejoConfig::new(
                    &request.base_url,
                    admin_token.expose_secret(),
                )
                .with_default_repo(&owner, &name);
                let forge = temper_forge::factory::new_forgejo_provisioning(forge_config);
                let repo = temper_provision::provision_with(&plan, forge.as_ref())
                    .await
                    .map_err(|error| format!("{owner}/{name}: {error}"))?;
                provisioned.push(repo);
            }

            Ok(ApplyPlanOutcome {
                provisioned,
                admin_token,
            })
        })
    }
}
