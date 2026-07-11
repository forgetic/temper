// SPDX-License-Identifier: MPL-2.0

//! Mutation-free Forge inspection for deployment planning.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use temper_config::ExposeSecret;
use temper_forge::{
    Forge, InspectionForge, IssueQuery, ItemListDetails, PullRequestQuery, Repository,
    RepositoryPath, WebhookStatus,
};
use temper_workflow::parse_metadata_block;

use crate::deployment::{DeploymentBundle, DesiredRepository};

/// Test/adapter seam for plan inspection. Repository calls are deliberately
/// separate so one unavailable repository cannot prevent later repositories
/// from being inspected.
pub trait DeploymentInspector {
    /// Inspect one desired repository without mutating the backend.
    fn inspect_repository(
        &mut self,
        bundle: &DeploymentBundle,
        repository: &DesiredRepository,
    ) -> Result<ForgeInspection, String>;

    /// Inspect deployment-wide user readiness.
    fn inspect_users(
        &mut self,
        _bundle: &DeploymentBundle,
        _users: &[String],
    ) -> Result<BTreeMap<String, bool>, String> {
        Ok(BTreeMap::new())
    }
}

/// Read-only snapshot for one repository.
#[derive(Clone, Debug, Default)]
pub struct ForgeInspection {
    pub inspected: bool,
    pub unavailable_reason: Option<String>,
    pub repository: Option<Repository>,
    pub labels: Vec<String>,
    pub webhooks: Vec<WebhookStatus>,
    pub ci_enabled: Option<bool>,
    pub metadata: MetadataInspection,
}

#[derive(Clone, Debug, Default)]
pub struct MetadataInspection {
    pub checked_artifacts: usize,
    pub invalid: Vec<String>,
}

pub(super) struct ForgePlanInspector {
    forge: Option<Arc<dyn InspectionForge>>,
    unavailable_reason: Option<String>,
}

impl ForgePlanInspector {
    pub(super) fn from_bundle(bundle: &DeploymentBundle) -> Self {
        if let Some(token) = &bundle.forge.admin_token {
            let config = temper_forge::config::ForgejoConfig::new(
                &bundle.forge.base_url,
                token.expose_secret(),
            );
            return Self {
                forge: Some(temper_forge::factory::new_forgejo_inspection(config)),
                unavailable_reason: None,
            };
        }

        if let (Some(admin_user), Some(admin_password)) = (
            bundle.forge.admin_user.as_deref(),
            bundle.forge.admin_password.as_ref(),
        ) {
            return Self {
                forge: Some(temper_forge::factory::new_forgejo_read_only_basic(
                    &bundle.forge.base_url,
                    admin_user,
                    admin_password.expose_secret(),
                )),
                unavailable_reason: None,
            };
        }

        Self {
            forge: None,
            unavailable_reason: Some(
                "forge inspection unavailable: no resolved admin token or admin Basic credentials"
                    .to_string(),
            ),
        }
    }
}

impl DeploymentInspector for ForgePlanInspector {
    fn inspect_repository(
        &mut self,
        bundle: &DeploymentBundle,
        desired: &DesiredRepository,
    ) -> Result<ForgeInspection, String> {
        let Some(forge) = self.forge.clone() else {
            return Ok(ForgeInspection {
                unavailable_reason: self.unavailable_reason.clone(),
                ..ForgeInspection::default()
            });
        };
        let repository = desired.plan.repo.clone();
        let declared_kinds: BTreeSet<String> = bundle
            .workflow
            .artifact_kinds()
            .iter()
            .map(|kind| kind.id.as_str().to_string())
            .collect();
        let runtime = temper_engine_io::build_runtime()?;
        temper_engine_io::runtime::block_on_runtime_with(&runtime, move |_cx, _handle| async move {
            inspect_forge_repository(
                forge.as_ref(),
                &repository.owner,
                &repository.name,
                &declared_kinds,
            )
            .await
            .map_err(|error| error.to_string())
        })
    }

    fn inspect_users(
        &mut self,
        _bundle: &DeploymentBundle,
        users: &[String],
    ) -> Result<BTreeMap<String, bool>, String> {
        let Some(forge) = self.forge.clone() else {
            return Err(self
                .unavailable_reason
                .clone()
                .unwrap_or_else(|| "forge inspection unavailable".to_string()));
        };
        let users = users.to_vec();
        let runtime = temper_engine_io::build_runtime()?;
        temper_engine_io::runtime::block_on_runtime_with(&runtime, move |_cx, _handle| async move {
            let mut readiness = BTreeMap::new();
            for login in users {
                let present = forge
                    .as_readiness()
                    .get_provisioned_user(&login)
                    .await
                    .map_err(|error| error.to_string())?
                    .is_some();
                readiness.insert(login, present);
            }
            Ok(readiness)
        })
    }
}

async fn inspect_forge_repository(
    forge: &dyn InspectionForge,
    owner: &str,
    name: &str,
    declared_kinds: &BTreeSet<String>,
) -> temper_forge::ForgeResult<ForgeInspection> {
    let path = RepositoryPath::new(owner, name);
    let repository = forge.as_forge().get_repository_by_path(&path).await?;
    let Some(repository) = repository else {
        return Ok(ForgeInspection {
            inspected: true,
            ..ForgeInspection::default()
        });
    };

    let labels = forge
        .as_forge()
        .list_labels(&repository.id)
        .await?
        .into_iter()
        .map(|label| label.name)
        .collect();
    let webhooks = forge
        .as_readiness()
        .list_webhook_statuses(&repository.id)
        .await?;
    let ci_enabled = forge
        .as_readiness()
        .repository_ci_enabled(&repository.id)
        .await?;
    let metadata = inspect_metadata(forge.as_forge(), &repository, declared_kinds).await?;

    Ok(ForgeInspection {
        inspected: true,
        repository: Some(repository),
        labels,
        webhooks,
        ci_enabled,
        metadata,
        unavailable_reason: None,
    })
}

async fn inspect_metadata(
    forge: &dyn Forge,
    repository: &Repository,
    declared_kinds: &BTreeSet<String>,
) -> temper_forge::ForgeResult<MetadataInspection> {
    let mut inspection = MetadataInspection::default();
    let issue_query = IssueQuery {
        details: ItemListDetails::summary(),
        ..IssueQuery::default()
    };
    for issue in forge.list_issues(&repository.id, issue_query).await? {
        inspection.checked_artifacts += 1;
        inspect_body_metadata(
            &mut inspection,
            &format!("issue #{}", issue.number.get()),
            &issue.body,
            declared_kinds,
        );
    }
    let pull_query = PullRequestQuery {
        details: ItemListDetails::summary(),
        ..PullRequestQuery::default()
    };
    for pull in forge.list_pull_requests(&repository.id, pull_query).await? {
        inspection.checked_artifacts += 1;
        inspect_body_metadata(
            &mut inspection,
            &format!("pull request #{}", pull.number.get()),
            &pull.body,
            declared_kinds,
        );
    }
    Ok(inspection)
}

fn inspect_body_metadata(
    inspection: &mut MetadataInspection,
    label: &str,
    body: &str,
    declared_kinds: &BTreeSet<String>,
) {
    match parse_metadata_block(body) {
        Ok(Some(metadata)) => {
            if let Some(kind) = metadata.kind {
                if !declared_kinds.contains(kind.as_str()) {
                    inspection.invalid.push(format!(
                        "{label}: metadata names undeclared artifact kind `{kind}`"
                    ));
                }
            }
        }
        Ok(None) => {}
        Err(error) => inspection
            .invalid
            .push(format!("{label}: malformed workflow metadata: {error}")),
    }
}

pub(super) fn desired_users(bundle: &DeploymentBundle) -> Vec<String> {
    let mut users = BTreeSet::new();
    if let Some(admin_user) = &bundle.forge.admin_user {
        users.insert(admin_user.clone());
    }
    for repository in &bundle.repositories {
        users.insert(repository.plan.automation_login.clone());
        for binding in &repository.plan.roles {
            users.insert(binding.user.handle.clone());
        }
    }
    users.into_iter().collect()
}
