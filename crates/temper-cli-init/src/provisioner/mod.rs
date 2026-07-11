// SPDX-License-Identifier: MPL-2.0

//! Provisioning seams and request/response models.

mod forgejo;

use std::path::PathBuf;

use temper_config::Secret;
use temper_provision::{ProvisionPlan, Provisioned};

use crate::deployment::DesiredRepository;

pub use forgejo::ForgejoProvisioner;

/// Secret-bearing inputs for the legacy init-local provisioning call.
#[derive(Debug, Clone)]
pub struct ProvisionRequest {
    pub base_url: String,
    pub admin_user: String,
    pub admin_password: Secret,
    pub owner: String,
    pub name: String,
    pub webhook_url: String,
    pub webhook_secret_file: PathBuf,
    pub workflow_path: Option<PathBuf>,
    pub existing_repo: bool,
}

/// Result of one init-local provisioning run.
pub struct ProvisionOutcome {
    pub provisioned: Provisioned,
    pub admin_token: Secret,
}

/// Adapter-bound request for every repository in a deployment.
///
/// This is constructed only at the provisioning boundary. Authentication
/// remains wrapped in [`Secret`]; repository plans contain raw backend values
/// only for the duration of this call.
#[derive(Clone)]
pub struct ApplyPlanRequest {
    pub base_url: String,
    pub admin_user: Option<String>,
    pub admin_password: Option<Secret>,
    pub admin_token: Option<Secret>,
    pub plans: Vec<ProvisionPlan>,
}

/// Result of executing all repository plans.
pub struct ApplyPlanOutcome {
    pub provisioned: Vec<Provisioned>,
    pub admin_token: Secret,
}

pub trait Provisioner {
    fn provision(&mut self, request: &ProvisionRequest) -> Result<ProvisionOutcome, String>;
}

pub trait ApplyProvisioner {
    fn provision_apply_plan(
        &mut self,
        request: &ApplyPlanRequest,
    ) -> Result<ApplyPlanOutcome, String>;
}

/// Builds the same no-seed desired repository model used by deployment apply,
/// then exposes it as a backend call for the init-local adapter.
pub(crate) fn build_init_plan(
    request: &ProvisionRequest,
    webhook: temper_forge::WebhookSpec,
) -> Result<ProvisionPlan, String> {
    let workflow = match &request.workflow_path {
        Some(path) => {
            temper_reference_delivery::load_workflow(path).map_err(|error| error.to_string())?
        }
        None => temper_reference_delivery::basic_delivery_workflow(),
    };
    let desired = DesiredRepository::from_workflow(
        &workflow,
        &request.owner,
        &request.name,
        request.existing_repo,
    )?;
    let mut plan = desired.expose_for_provisioning(None);
    plan.webhook = Some(webhook);
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use temper_config::Secret;

    use super::*;

    #[test]
    fn init_plan_requests_no_reference_delivery_ci_seed_commits() {
        let request = ProvisionRequest {
            base_url: "http://forge.local:3000".to_string(),
            admin_user: "root".to_string(),
            admin_password: Secret::from("admin-pass"),
            owner: "acme".to_string(),
            name: "service".to_string(),
            webhook_url: "http://127.0.0.1:8314/forgejo/webhook".to_string(),
            webhook_secret_file: PathBuf::from("webhook-secret"),
            workflow_path: None,
            existing_repo: false,
        };
        let webhook = temper_forge::WebhookSpec {
            url: request.webhook_url.clone(),
            secret: "secret".to_string(),
            events: temper_forge::WebhookEvents::All,
        };

        let plan = build_init_plan(&request, webhook).expect("init plan builds");

        assert!(plan.seed_commits.is_empty());
        assert!(!plan.repository_auto_init);
    }
}
