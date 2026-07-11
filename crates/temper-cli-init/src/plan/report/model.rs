// SPDX-License-Identifier: MPL-2.0

use serde::Serialize;

/// Version of the `temper plan --format json` report contract.
pub const REPORT_VERSION: u32 = 1;

/// Top-level JSON/human report for `temper plan`.
#[derive(Clone, Debug, Serialize)]
pub struct DeploymentPlanReport {
    pub report_version: u32,
    pub status: String,
    pub result: String,
    pub loaded: LoadedReport,
    pub deployment: DeploymentReport,
    pub forge: ForgeReport,
    pub repositories: Vec<RepositoryPlanReport>,
    /// Compatibility projection emitted only for a one-repository deployment.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<RepositoryReport>,
    pub workflow: WorkflowReport,
    /// Compatibility projection emitted only for a one-repository deployment.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<LabelReport>>,
    /// Compatibility projection emitted only for a one-repository deployment.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub webhook: Option<WebhookReport>,
    pub identities: IdentityReport,
    /// Compatibility projection emitted only for a one-repository deployment.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<MetadataReport>,
    pub findings: Vec<PlanFinding>,
}

impl DeploymentPlanReport {
    pub(in crate::plan) fn has_error_findings(&self) -> bool {
        self.findings
            .iter()
            .any(|finding| finding.severity == "error")
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct LoadedReport {
    pub config_path: Option<String>,
    pub credentials_path: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DeploymentReport {
    pub name: Option<String>,
    pub topology: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ForgeReport {
    pub kind: String,
    pub url: String,
    pub inspected: bool,
    pub inspection_note: Option<String>,
}

/// Repository-scoped report. Every desired repository has exactly one entry.
#[derive(Clone, Debug, Serialize)]
pub struct RepositoryPlanReport {
    pub repository: RepositoryReport,
    pub labels: Vec<LabelReport>,
    pub webhook: WebhookReport,
    pub metadata: MetadataReport,
    pub findings: Vec<PlanFinding>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RepositoryReport {
    pub path: String,
    pub existing_repo_required: bool,
    pub exists: Option<bool>,
    pub id: Option<String>,
    pub default_branch: String,
    pub ci_enabled: Option<bool>,
    pub action: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct WorkflowReport {
    pub name: String,
    pub path: Option<String>,
    pub roles: Vec<String>,
    pub queued_roles: Vec<String>,
    pub labels: usize,
    pub artifact_kinds: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct LabelReport {
    pub name: String,
    pub present: Option<bool>,
    pub action: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct WebhookReport {
    pub url: String,
    pub secret: String,
    pub events: String,
    pub configured: Option<bool>,
    pub action: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct IdentityReport {
    pub admin: AdminIdentityReport,
    pub automation: AutomationIdentityReport,
    pub roles: Vec<RoleIdentityReport>,
    pub users: Vec<UserReadinessReport>,
}

#[derive(Clone, Debug, Serialize)]
pub struct AdminIdentityReport {
    pub key: String,
    pub user: String,
    pub password: String,
    pub token: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct AutomationIdentityReport {
    pub user: String,
    pub token: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct RoleIdentityReport {
    pub role: String,
    pub user: String,
    pub email: String,
    pub token: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct UserReadinessReport {
    pub user: String,
    pub present: Option<bool>,
    pub action: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct MetadataReport {
    pub compatible: bool,
    pub checked_artifacts: usize,
    pub invalid: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PlanFinding {
    pub severity: String,
    pub category: String,
    pub message: String,
}
