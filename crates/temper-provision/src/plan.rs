//! Distilling a [`ProvisionPlan`] from a validated workflow.

use temper_forge::{AccessScope, RepositoryPath};
use temper_workflow::ValidatedWorkflow;

use crate::error::{ProvisionError, Result};
use crate::intake::{has_default_issue_kind, intake_labels};
use crate::model::{LabelSpec, ProvisionOptions, ProvisionPlan};

/// Default label color applied to every workflow label, preserving the color the
/// Forgejo adapter previously upserted.
const LABEL_COLOR: &str = "#ededed";

impl ProvisionPlan {
    /// Distills a backend-agnostic provisioning plan from a validated workflow.
    ///
    /// Labels come from the workflow's compiled label manifest (one upsert per
    /// declared label, in declaration order). Every other knob — the role
    /// bindings, the automation login, the token scopes, the seed commits, the
    /// webhook, and the intake seed — is supplied by the caller through
    /// [`ProvisionOptions`], so this never reaches into any reference-delivery
    /// default; the backend adapter passes those in.
    pub fn from_workflow(
        workflow: &ValidatedWorkflow,
        repo: RepositoryPath,
        default_branch: String,
        access: AccessScope,
        options: ProvisionOptions,
    ) -> Result<Self> {
        let compiled = workflow.compile();
        let labels: Vec<LabelSpec> = compiled
            .labels()
            .labels()
            .iter()
            .map(|label| LabelSpec {
                name: label.id.to_string(),
                color: Some(LABEL_COLOR.to_string()),
                description: None,
            })
            .collect();

        // Workflow-declared labels and any caller-supplied labels are both
        // upserted; the workflow labels come first to preserve declaration order.
        let mut all_labels = labels;
        all_labels.extend(options.labels);

        // Resolve the intake issue's labels up front, validating that the
        // workflow actually has a queued entry point when an intake seed was
        // requested. An empty label set is valid only when the workflow declares
        // a default (catch-all) issue kind.
        let intake_labels = if options.intake.is_some() {
            let resolved = intake_labels(workflow);
            if resolved.is_empty() && !has_default_issue_kind(workflow) {
                return Err(ProvisionError::Shape {
                    what: "intake labels".into(),
                    detail: "workflow declares no queued entry issue artifact".into(),
                });
            }
            resolved
        } else {
            Vec::new()
        };

        Ok(ProvisionPlan {
            repo,
            default_branch,
            roles: options.roles,
            automation_login: options.automation_login,
            password: options.password,
            access,
            token_scopes: options.token_scopes,
            labels: all_labels,
            existing_repo: options.existing_repo,
            repository_auto_init: options.repository_auto_init,
            webhook: options.webhook,
            intake: options.intake,
            intake_labels,
            seed_commits: options.seed_commits,
        })
    }
}
