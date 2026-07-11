// SPDX-License-Identifier: MPL-2.0

//! Provisioning seam and deployment-wide request/response models.

use temper_config::Secret;
use temper_provision::{ProvisionPlan, Provisioned};

mod forgejo;
pub use forgejo::ForgejoProvisioner;

/// Adapter-bound request for every repository in a deployment.
#[derive(Clone)]
pub struct ApplyPlanRequest {
    pub base_url: String,
    pub admin_user: Option<String>,
    pub admin_password: Option<Secret>,
    pub admin_token: Option<Secret>,
    pub plans: Vec<ProvisionPlan>,
}

/// Result produced only after executing the complete repository plan.
pub struct ApplyPlanOutcome {
    pub provisioned: Vec<Provisioned>,
    pub admin_token: Secret,
}

pub trait ApplyProvisioner {
    fn provision_apply_plan(
        &mut self,
        request: &ApplyPlanRequest,
    ) -> Result<ApplyPlanOutcome, String>;
}
