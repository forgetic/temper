// SPDX-License-Identifier: MPL-2.0

//! Shared broad role-wake feeding across configured subscribers.

use std::collections::{BTreeMap, BTreeSet};

use temper_forge::{Forge, HintArtifactKind, Repository};
use temper_runner::{ArtifactAddress, ScanError, scan_roles_wake};
use temper_workflow::{CompiledWorkflow, QueueId, RoleId, ValidatedWorkflow};

use super::recovery::recover_advanced_pull_request_assignments_for_roles;
use super::{
    EnrichOutcome, enrich_work_item_job_inner, enrichment_failure_log_line, job_from_work_item,
    prepare_interrupted_ci_recovery_item, skip_log_line,
};

/// Enqueued selections from one broad wake, retained so an exact CI hint that
/// coalesced into the broad lane can still report its selected queue and role.
pub(crate) struct BroadRoleFeedResult {
    pub(crate) enqueued_work: Vec<(ArtifactAddress, QueueId, RoleId)>,
}

/// Runs one recovery-inclusive candidate pass for the union of `roles`, then
/// groups enrichment and broad pending reconciliation by subscriber.
///
/// Unlike role-at-a-time scans, candidate list and gate-signal reads shared by
/// overlapping role queues happen once.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn enqueue_scanned_roles_wake<F: Forge + ?Sized>(
    daemon: &crate::Daemon,
    forge: &F,
    repository: &Repository,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    now: chrono::DateTime<chrono::Utc>,
    roles: &[RoleId],
) -> Result<BroadRoleFeedResult, ScanError> {
    let repo = &repository.id;
    recover_advanced_pull_request_assignments_for_roles(daemon, forge, repo, workflow, roles)
        .await?;

    let repo_label = format!("{}/{}", repository.owner, repository.name);
    let items = scan_roles_wake(forge, repo, workflow, compiled, now, roles).await?;
    let mut current_job_ids = roles
        .iter()
        .cloned()
        .map(|role| (role, BTreeSet::new()))
        .collect::<BTreeMap<_, _>>();
    let mut enqueued_work = Vec::new();

    for item in &items {
        if !prepare_interrupted_ci_recovery_item(forge, repository, workflow, compiled, now, item)
            .await?
        {
            continue;
        }
        let mut job = job_from_work_item(&repo_label, item);
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
            None,
        )
        .await
        {
            Ok(EnrichOutcome::Enriched) => {
                current_job_ids
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
                enqueued_work.push((
                    match item.target {
                        temper_workflow::ArtifactSource::Issue { number } => {
                            ArtifactAddress::new(HintArtifactKind::Issue, number)
                        }
                        temper_workflow::ArtifactSource::PullRequest { number } => {
                            ArtifactAddress::new(HintArtifactKind::PullRequest, number)
                        }
                    },
                    item.queue.clone(),
                    item.role.clone(),
                ));
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

    for role in roles {
        daemon
            .reconcile_pending_role_jobs(
                &repo_label,
                role.as_str(),
                current_job_ids.remove(role).unwrap_or_default(),
            )
            .await;
    }
    Ok(BroadRoleFeedResult { enqueued_work })
}
