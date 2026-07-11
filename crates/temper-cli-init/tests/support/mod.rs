// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeMap;
use std::path::Path;

use temper_cli_common::LoadOptions;
use temper_cli_init::{ApplyPlanOutcome, ApplyPlanRequest, ApplyProvisioner};
use temper_config::{NoEnv, ResolveOptions, Secret};
use temper_forge::RepositoryId;
use temper_provision::{Provisioned, RoleIdentity};

#[derive(Default)]
pub struct StubProvisioner {
    pub seen: Option<ApplyPlanRequest>,
}

impl ApplyProvisioner for StubProvisioner {
    fn provision_apply_plan(
        &mut self,
        request: &ApplyPlanRequest,
    ) -> Result<ApplyPlanOutcome, String> {
        self.seen = Some(request.clone());
        successful_outcome(request)
    }
}

#[derive(Default)]
pub struct RecordingProvisioner {
    pub calls: Vec<ApplyPlanRequest>,
    pub fail_repo: Option<String>,
}

impl ApplyProvisioner for RecordingProvisioner {
    fn provision_apply_plan(
        &mut self,
        request: &ApplyPlanRequest,
    ) -> Result<ApplyPlanOutcome, String> {
        self.calls.push(request.clone());
        if let Some(failed) = &self.fail_repo {
            // Walk in plan order so this models a later repository failing after
            // an earlier Forge mutation, while returning no partial outcome.
            for plan in &request.plans {
                let path = format!("{}/{}", plan.repo.owner, plan.repo.name);
                if &path == failed {
                    return Err(format!("{path}: simulated failure"));
                }
            }
        }
        successful_outcome(request)
    }
}

fn successful_outcome(request: &ApplyPlanRequest) -> Result<ApplyPlanOutcome, String> {
    let identity = |user: &str| RoleIdentity {
        user: user.to_string(),
        email: format!("{user}@example.invalid"),
        token: format!("token-{user}"),
        password: format!("pw-{user}"),
    };
    let provisioned = request
        .plans
        .iter()
        .map(|plan| {
            let mut roles = BTreeMap::new();
            for binding in &plan.roles {
                roles.insert(binding.role.clone(), identity(&binding.user.handle));
            }
            Provisioned {
                owner: plan.repo.owner.clone(),
                name: plan.repo.name.clone(),
                repository: RepositoryId::new(format!("{}/{}", plan.repo.owner, plan.repo.name)),
                roles,
                automation: identity(&plan.automation_login),
            }
        })
        .collect();
    Ok(ApplyPlanOutcome {
        provisioned,
        admin_token: Secret::from("admin-rest-token"),
    })
}

#[allow(dead_code)]
pub fn resolve_generated_bundle_non_strict(
    config_path: &Path,
    credentials_path: &Path,
) -> temper_config::Resolved {
    let config = temper_config::Config::load(config_path).expect("config parses");
    let credentials =
        temper_config::Credentials::load(credentials_path).expect("credentials parse");
    let mut options = config_path
        .parent()
        .map(ResolveOptions::from_config_base_dir)
        .unwrap_or_default();
    options.validate_secret_references = false;
    temper_config::resolve_with_options(&config, &credentials, &NoEnv, &options)
        .expect("generated bundle resolves non-strictly")
}

#[allow(dead_code)]
pub fn load_generated_bundle(
    config_path: &Path,
    credentials_path: &Path,
) -> temper_config::Resolved {
    temper_config::load_with_env(
        &LoadOptions {
            config: Some(config_path.to_path_buf()),
            credentials: Some(credentials_path.to_path_buf()),
        },
        &NoEnv,
    )
    .expect("generated bundle parses and resolves")
    .0
}
