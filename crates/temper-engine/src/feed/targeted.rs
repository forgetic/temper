// SPDX-License-Identifier: MPL-2.0

//! Item-scoped role-feed mapping and reusable targeted snapshots.

use std::collections::{BTreeMap, BTreeSet};

use temper_forge::{Forge, Repository, RepositoryId};
use temper_protocol_context::{
    ArtifactContextBundle, ArtifactReference as ContextArtifactReference,
    ArtifactRepository as ContextRepository, ArtifactSnapshot as ContextArtifactSnapshot,
    ArtifactType as ContextArtifactType,
};
use temper_protocol_worker::JobArtifactSnapshot;
use temper_runner::{
    ArtifactAddress, ScanError, TargetedArtifactSnapshot, WorkItem, targeted_role_work_items,
};
use temper_workflow::{ClassifiedArtifact, CompiledWorkflow, RoleId, ValidatedWorkflow};

use super::{
    EnrichOutcome, WorkItemJob, enrich_work_item_job_inner, enrichment_failure_log_line,
    job_from_work_item, skip_log_line,
};

/// Exact current job identities produced by one item-scoped role feed.
///
/// Empty role sets are meaningful: callers may reconcile pending work for this
/// one artifact and role without treating the partial view as repository-wide.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct TargetedRoleFeedResult {
    pub artifact: ArtifactAddress,
    pub enqueued: usize,
    pub current_job_ids: BTreeMap<RoleId, BTreeSet<String>>,
}

pub(super) struct TargetedEnrichment<'a> {
    pub repository: &'a Repository,
    pub snapshot: &'a TargetedArtifactSnapshot,
    pub classified: &'a ClassifiedArtifact,
}

pub(super) fn job_snapshot(snapshot: &TargetedArtifactSnapshot) -> Option<JobArtifactSnapshot> {
    if !snapshot.is_open() {
        return None;
    }
    Some(match snapshot {
        TargetedArtifactSnapshot::Issue(issue) => JobArtifactSnapshot {
            number: issue.number.get(),
            title: issue.title.clone(),
            body: issue.body.clone(),
            labels: issue.labels.clone(),
            state: format!("{:?}", issue.state),
        },
        TargetedArtifactSnapshot::PullRequest(pull_request) => JobArtifactSnapshot {
            number: pull_request.number.get(),
            title: pull_request.title.clone(),
            body: pull_request.body.clone(),
            labels: pull_request.labels.clone(),
            state: format!("{:?}", pull_request.state),
        },
    })
}

pub(super) fn context_snapshot(
    repo_label: &str,
    snapshot: &TargetedArtifactSnapshot,
    classified: &ClassifiedArtifact,
) -> ContextArtifactSnapshot {
    let (artifact_type, number, title, body, labels, state) = match snapshot {
        TargetedArtifactSnapshot::Issue(issue) => (
            ContextArtifactType::Issue,
            issue.number.get(),
            issue.title.clone(),
            issue.body.clone(),
            issue.labels.clone(),
            format!("{:?}", issue.state).to_lowercase(),
        ),
        TargetedArtifactSnapshot::PullRequest(pull_request) => (
            ContextArtifactType::PullRequest,
            pull_request.number.get(),
            pull_request.title.clone(),
            pull_request.body.clone(),
            pull_request.labels.clone(),
            format!("{:?}", pull_request.state).to_lowercase(),
        ),
    };
    let mut labels = labels;
    labels.sort();
    labels.dedup();
    ContextArtifactSnapshot {
        artifact: ContextArtifactReference {
            repository: ContextRepository {
                id: match snapshot {
                    TargetedArtifactSnapshot::Issue(issue) => issue.repo_id.to_string(),
                    TargetedArtifactSnapshot::PullRequest(pull_request) => {
                        pull_request.repo_id.to_string()
                    }
                },
                path: repo_label.to_string(),
            },
            artifact_type,
            number,
        },
        title,
        body,
        labels,
        state,
        workflow_kind: Some(classified.kind.to_string()),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn resolve_artifact_context<F: Forge + ?Sized>(
    forge: &F,
    repository: &Repository,
    job_repo: &str,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
    item: &WorkItem,
    action: &str,
    service: Option<&crate::ArtifactContextBundleService>,
    targeted: Option<&TargetedEnrichment<'_>>,
) -> Result<ArtifactContextBundle, ScanError> {
    match (service, targeted) {
        (Some(service), Some(targeted)) => service
            .resolve_with_primary(
                repo,
                action,
                context_snapshot(job_repo, targeted.snapshot, targeted.classified),
                targeted.classified.clone(),
            )
            .await
            .map_err(|error| ScanError::InvalidWorkflow(error.to_string())),
        (Some(service), None) => service
            .resolve(repo, item.target, action)
            .await
            .map_err(|error| ScanError::InvalidWorkflow(error.to_string())),
        (None, targeted) => {
            let catalog = crate::ConfiguredRepositoryCatalog::single(
                repository.id.clone(),
                temper_forge::RepositoryPath::new(
                    repository.owner.clone(),
                    repository.name.clone(),
                ),
                "",
            );
            match targeted {
                Some(targeted) => crate::resolve_initial_artifact_context_for_action_with_primary(
                    forge,
                    &catalog,
                    workflow,
                    repo,
                    action,
                    context_snapshot(job_repo, targeted.snapshot, targeted.classified),
                    targeted.classified.clone(),
                    crate::ArtifactContextPolicy::default(),
                )
                .await
                .map_err(|error| ScanError::InvalidWorkflow(error.to_string())),
                None => crate::resolve_initial_artifact_context_for_action_with_policy(
                    forge,
                    &catalog,
                    workflow,
                    repo,
                    item.target,
                    action,
                    crate::ArtifactContextPolicy::default(),
                )
                .await
                .map_err(|error| ScanError::InvalidWorkflow(error.to_string())),
            }
        }
    }
}

/// Evaluates and enqueues role work for exactly one artifact.
///
/// This path deliberately performs no advanced-PR recovery, candidate-list
/// planning, or repository-wide pending reconciliation. The returned job ids
/// are scoped to `artifact` and each requested role so the integration layer
/// can reconcile only that exact partial view.
#[allow(clippy::too_many_arguments)]
pub async fn enqueue_targeted_role_work<F: Forge + ?Sized>(
    daemon: &crate::Daemon,
    forge: &F,
    repository: &Repository,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    now: chrono::DateTime<chrono::Utc>,
    artifact: ArtifactAddress,
    roles: &[RoleId],
) -> Result<TargetedRoleFeedResult, ScanError> {
    let repo = &repository.id;
    let repo_label = format!("{}/{}", repository.owner, repository.name);
    let mut result = TargetedRoleFeedResult {
        artifact,
        enqueued: 0,
        current_job_ids: roles
            .iter()
            .cloned()
            .map(|role| (role, BTreeSet::new()))
            .collect(),
    };
    let Some(scan) =
        targeted_role_work_items(forge, repo, workflow, compiled, artifact, roles, now).await?
    else {
        return Ok(result);
    };

    for item in &scan.work_items {
        let mut job: WorkItemJob = job_from_work_item(&repo_label, item);
        match enrich_work_item_job_inner(
            forge,
            repo,
            item,
            &mut job,
            workflow,
            compiled,
            false,
            daemon.artifact_context.as_deref(),
            Some(repository),
            Some(TargetedEnrichment {
                repository,
                snapshot: &scan.snapshot,
                classified: &scan.classified,
            }),
        )
        .await
        {
            Ok(EnrichOutcome::Enriched) => {
                result
                    .current_job_ids
                    .entry(item.role.clone())
                    .or_default()
                    .insert(job.job_id.clone());
                daemon
                    .enqueue_job(
                        job.job_id,
                        job.role,
                        job.repo,
                        job.artifact,
                        job.job_payload,
                    )
                    .await;
                result.enqueued += 1;
            }
            Ok(
                outcome @ (EnrichOutcome::SkipTerminalArtifact
                | EnrichOutcome::SkipAttentionArtifact
                | EnrichOutcome::SkipExistingPullRequest),
            ) => tracing::debug!("{}", skip_log_line(&repo_label, &item.role, item, outcome)),
            Err(error) => tracing::debug!(
                "{}",
                enrichment_failure_log_line(&repo_label, &item.role, item, &error)
            ),
        }
    }
    Ok(result)
}
