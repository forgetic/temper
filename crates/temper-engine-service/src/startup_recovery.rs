// SPDX-License-Identifier: MPL-2.0

//! Durable assignment inventory and fresh-state startup convergence.

use std::collections::BTreeMap;

use chrono::Utc;
use temper_engine::Daemon;
use temper_forge::{
    CreateComment, Forge, ForgeError, ForgeResult, IssueQuery, IssueState, PullRequest,
    PullRequestQuery, PullRequestState, RepositoryId, UpdateIssue, UpdatePullRequest,
};
use temper_workflow::{
    ArtifactKindId, ArtifactSource, Classifier, CompiledWorkflow, DurableAssignment, Effect,
    LeaseManager, LeasePolicy, METADATA_BEGIN, RelationKind, ValidatedWorkflow,
    parse_metadata_block,
};

#[derive(Clone)]
pub struct RecoveredClaim {
    repo: RepositoryId,
    target: ArtifactSource,
    assignment: DurableAssignment,
    kind: ArtifactKindId,
    queue_labels: Vec<String>,
    claim_labels: Vec<String>,
    assignment_valid: bool,
    queue_known: bool,
}

pub async fn stage_startup_assignments(
    daemon: &Daemon,
    forge: &dyn Forge,
    repos: &[RepositoryId],
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    policy: LeasePolicy,
    now: chrono::DateTime<Utc>,
) -> Result<BTreeMap<String, RecoveredClaim>, String> {
    const MAX_RECOVERY_CANDIDATES: usize = 1_000;
    let artifact_context = daemon.artifact_context_service();
    let mut candidates = Vec::new();
    for repo in repos {
        let issues = forge
            .list_issues(
                repo,
                IssueQuery {
                    state: Some(IssueState::Open),
                    body_contains: Some(METADATA_BEGIN.to_string()),
                    ..IssueQuery::default()
                },
            )
            .await
            .map_err(|error| format!("startup issue inventory failed for {repo}: {error}"))?;
        candidates.extend(issues.into_iter().map(|issue| {
            (
                repo.clone(),
                ArtifactSource::Issue {
                    number: issue.number,
                },
                issue.body,
            )
        }));
        let pull_requests = forge
            .list_pull_requests(
                repo,
                PullRequestQuery {
                    state: Some(PullRequestState::Open),
                    body_contains: Some(METADATA_BEGIN.to_string()),
                    ..PullRequestQuery::default()
                },
            )
            .await;
        let pull_requests = startup_pull_inventory(repo, pull_requests)?;
        candidates.extend(pull_requests.into_iter().map(|pull_request| {
            (
                repo.clone(),
                ArtifactSource::PullRequest {
                    number: pull_request.number,
                },
                pull_request.body,
            )
        }));
        if candidates.len() > MAX_RECOVERY_CANDIDATES {
            return Err(format!(
                "startup recovery candidate limit exceeded ({MAX_RECOVERY_CANDIDATES})"
            ));
        }
    }
    candidates.sort_by_key(|(repo, target, _)| {
        let (kind, number) = match target {
            ArtifactSource::Issue { number } => (0_u8, number.get()),
            ArtifactSource::PullRequest { number } => (1_u8, number.get()),
        };
        (repo.clone(), kind, number)
    });

    let mut prepared = Vec::new();
    for (repo, target, body) in candidates {
        let metadata = match parse_metadata_block(&body) {
            Ok(Some(metadata)) => metadata,
            Ok(None) => continue,
            Err(error) => {
                quarantine_target(
                    forge,
                    &repo,
                    target,
                    &format!("malformed workflow metadata: {error}"),
                )
                .await?;
                continue;
            }
        };
        let Some(assignment) = metadata.assignment else {
            continue;
        };
        let Some(kind) = metadata.kind else {
            quarantine_invalid_assignment(
                forge,
                policy,
                &repo,
                target,
                &assignment,
                "durable assignment is missing workflow kind",
            )
            .await?;
            continue;
        };
        let queue = assignment.queue.as_deref().and_then(|queue| {
            compiled
                .queues()
                .iter()
                .find(|candidate| candidate.id.as_str() == queue)
        });
        let queue_known = queue.is_some();
        let queue_labels = queue
            .map(|queue| {
                let mut labels = queue
                    .labels
                    .iter()
                    .map(|label| label.as_str().to_string())
                    .collect::<Vec<_>>();
                if let Some(branch) = queue.any_of.iter().find(|branch| {
                    branch.labels.iter().all(|label| {
                        assignment
                            .pre_claim_labels
                            .iter()
                            .any(|present| present == label.as_str())
                    })
                }) {
                    for label in &branch.labels {
                        if !labels.iter().any(|present| present == label.as_str()) {
                            labels.push(label.as_str().to_string());
                        }
                    }
                }
                labels
            })
            .unwrap_or_default();
        let assigned_transition = assignment.action.as_deref().and_then(|action| {
            workflow
                .transitions()
                .iter()
                .find(|transition| transition.id.as_str() == action)
        });
        let assignment_valid = assigned_transition.is_some_and(|transition| {
            transition.artifact == kind
                && assignment
                    .role
                    .as_ref()
                    .is_some_and(|role| transition.roles.contains(role))
        });
        let claim_labels = assigned_transition
            .map(|transition| {
                transition
                    .effects
                    .iter()
                    .filter_map(|effect| match effect {
                        Effect::AddLabel(label) => Some(label.as_str().to_string()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let Some(job_id) = assignment
            .job_id
            .clone()
            .filter(|value| !value.trim().is_empty())
        else {
            quarantine_invalid_assignment(
                forge,
                policy,
                &repo,
                target,
                &assignment,
                "durable assignment is missing job id",
            )
            .await?;
            continue;
        };
        let Some(worker_id) = assignment
            .worker_id
            .clone()
            .filter(|value| !value.trim().is_empty())
        else {
            quarantine_invalid_assignment(
                forge,
                policy,
                &repo,
                target,
                &assignment,
                "durable assignment is missing worker id",
            )
            .await?;
            continue;
        };
        let Some(prior_boot) = assignment
            .daemon_boot_id
            .clone()
            .filter(|value| !value.trim().is_empty())
        else {
            quarantine_invalid_assignment(
                forge,
                policy,
                &repo,
                target,
                &assignment,
                "durable assignment is missing daemon boot id",
            )
            .await?;
            continue;
        };
        let Some(expires_at) = assignment
            .expires_at
            .or_else(|| metadata.lease.as_ref().map(|lease| lease.expires_at))
        else {
            quarantine_invalid_assignment(
                forge,
                policy,
                &repo,
                target,
                &assignment,
                "durable assignment is missing expiry",
            )
            .await?;
            continue;
        };
        let claim = RecoveredClaim {
            repo: repo.clone(),
            target,
            assignment: assignment.clone(),
            kind,
            queue_labels,
            claim_labels,
            assignment_valid,
            queue_known,
        };
        if expires_at <= now {
            converge_startup_claim(forge, policy, workflow, &claim).await?;
            continue;
        }
        let reconstructed = if let Some(service) = artifact_context.as_deref() {
            temper_engine::recovered_job_from_assignment_with_artifact_context(
                forge,
                &repo,
                target,
                &assignment,
                workflow,
                compiled,
                service,
            )
            .await
        } else {
            temper_engine::recovered_job_from_assignment(
                forge,
                &repo,
                target,
                &assignment,
                workflow,
                compiled,
            )
            .await
        };
        let job = match reconstructed {
            Ok(job) => job,
            Err(reason) => {
                tracing::warn!(job_id = %job_id, %reason, "quarantining impossible durable assignment");
                quarantine_invalid_assignment(forge, policy, &repo, target, &assignment, &reason)
                    .await?;
                continue;
            }
        };
        prepared.push((
            job_id,
            claim,
            temper_engine::RecoveredJob {
                job_id: job.job_id,
                worker_id,
                role: job.role,
                repo: job.repo,
                artifact: job.artifact,
                job_payload: job.job_payload,
            },
            prior_boot,
        ));
    }

    let mut job_id_counts = BTreeMap::<String, usize>::new();
    for (job_id, _, _, _) in &prepared {
        *job_id_counts.entry(job_id.clone()).or_default() += 1;
    }
    let mut staged = BTreeMap::new();
    for (job_id, claim, job, prior_boot) in prepared {
        if job_id_counts.get(&job_id).copied().unwrap_or_default() > 1 {
            quarantine_invalid_assignment(
                forge,
                policy,
                &claim.repo,
                claim.target,
                &claim.assignment,
                "multiple durable assignments claim the same job id",
            )
            .await?;
            continue;
        }
        daemon
            .stage_recovered_job(job, prior_boot)
            .await
            .map_err(|error| format!("could not stage recovered claim {job_id}: {error:?}"))?;
        staged.insert(job_id, claim);
    }
    Ok(staged)
}

fn startup_pull_inventory(
    repo: &RepositoryId,
    result: ForgeResult<Vec<PullRequest>>,
) -> Result<Vec<PullRequest>, String> {
    match result {
        Ok(pull_requests) => Ok(pull_requests),
        // Forgejo reports its /pulls collection as 404 until a repository has
        // a Git history. The issue inventory immediately before this call
        // succeeded, so the repository itself is known to exist and an absent
        // PR collection is equivalent to an empty recovery inventory.
        Err(ForgeError::NotFound(error)) => {
            tracing::debug!(%repo, %error, "startup PR collection is not available yet");
            Ok(Vec::new())
        }
        Err(error) => Err(format!("startup PR inventory failed for {repo}: {error}")),
    }
}

const STARTUP_RECOVERY_AUDIT_MARKER: &str =
    "<!-- temper:comment-key=startup_assignment_recovery -->";

async fn quarantine_invalid_assignment(
    forge: &dyn Forge,
    policy: LeasePolicy,
    repo: &RepositoryId,
    target: ArtifactSource,
    assignment: &DurableAssignment,
    reason: &str,
) -> Result<(), String> {
    LeaseManager::new(forge, policy)
        .quarantine_assignment(repo, target, assignment)
        .await
        .map_err(|error| format!("could not quarantine impossible claim on {repo}: {error}"))?;
    quarantine_target(forge, repo, target, reason).await
}

async fn quarantine_target(
    forge: &dyn Forge,
    repo: &RepositoryId,
    target: ArtifactSource,
    reason: &str,
) -> Result<(), String> {
    let audit_body = format!(
        "Startup recovery could not safely converge a durable assignment. The artifact was parked for human inspection.\n\nReason: {reason}\n\n{STARTUP_RECOVERY_AUDIT_MARKER}"
    );
    match target {
        ArtifactSource::Issue { number } => {
            let issue = forge
                .get_issue_by_number(repo, number)
                .await
                .map_err(|error| format!("could not load quarantined issue on {repo}: {error}"))?
                .ok_or_else(|| format!("quarantined issue {number} disappeared from {repo}"))?;
            if !issue.labels.iter().any(|label| label == "needs-human") {
                forge
                    .update_issue(
                        &issue.id,
                        UpdateIssue {
                            add_labels: vec!["needs-human".to_string()],
                            ..UpdateIssue::default()
                        },
                    )
                    .await
                    .map_err(|error| format!("could not label quarantined issue: {error}"))?;
            }
            let comments = forge
                .list_issue_comments(&issue.id)
                .await
                .map_err(|error| format!("could not inspect recovery audit comments: {error}"))?;
            if !comments
                .iter()
                .any(|comment| comment.body.contains(STARTUP_RECOVERY_AUDIT_MARKER))
            {
                forge
                    .add_issue_comment(&issue.id, CreateComment { body: audit_body })
                    .await
                    .map_err(|error| format!("could not record recovery audit: {error}"))?;
            }
        }
        ArtifactSource::PullRequest { number } => {
            let pull_request = forge
                .get_pull_request_by_number(repo, number)
                .await
                .map_err(|error| format!("could not load quarantined PR on {repo}: {error}"))?
                .ok_or_else(|| format!("quarantined PR {number} disappeared from {repo}"))?;
            if !pull_request
                .labels
                .iter()
                .any(|label| label == "needs-human")
            {
                forge
                    .update_pull_request(
                        &pull_request.id,
                        UpdatePullRequest {
                            add_labels: vec!["needs-human".to_string()],
                            ..UpdatePullRequest::default()
                        },
                    )
                    .await
                    .map_err(|error| format!("could not label quarantined PR: {error}"))?;
            }
            let comments = forge
                .list_pull_request_comments(&pull_request.id)
                .await
                .map_err(|error| format!("could not inspect recovery audit comments: {error}"))?;
            if !comments
                .iter()
                .any(|comment| comment.body.contains(STARTUP_RECOVERY_AUDIT_MARKER))
            {
                forge
                    .add_pull_request_comment(&pull_request.id, CreateComment { body: audit_body })
                    .await
                    .map_err(|error| format!("could not record recovery audit: {error}"))?;
            }
        }
    }
    Ok(())
}

async fn converge_startup_claim(
    forge: &dyn Forge,
    policy: LeasePolicy,
    workflow: &ValidatedWorkflow,
    claim: &RecoveredClaim,
) -> Result<(), String> {
    if !claim.assignment_valid {
        return quarantine_invalid_assignment(
            forge,
            policy,
            &claim.repo,
            claim.target,
            &claim.assignment,
            "durable assignment action is missing, unknown, or unauthorized",
        )
        .await;
    }
    if !claim.queue_known {
        return quarantine_invalid_assignment(
            forge,
            policy,
            &claim.repo,
            claim.target,
            &claim.assignment,
            "durable assignment names an unknown queue",
        )
        .await;
    }

    if matches!(claim.target, ArtifactSource::PullRequest { .. }) {
        let recovered = temper_engine::recover_advanced_pull_request_assignment_from_durable(
            forge,
            &claim.repo,
            claim.target,
            &claim.assignment,
            claim.kind.clone(),
            workflow,
        )
        .await
        .map_err(|error| format!("could not recover advanced PR head: {error}"))?;
        if recovered {
            return Ok(());
        }
        return LeaseManager::new(forge, policy)
            .rollback_assignment(&claim.repo, claim.target, &claim.assignment)
            .await
            .map_err(|error| format!("could not converge abandoned PR assignment: {error}"));
    }

    let ArtifactSource::Issue { number } = claim.target else {
        unreachable!("pull requests returned above")
    };
    let issue = forge
        .get_issue_by_number(&claim.repo, number)
        .await
        .map_err(|error| format!("could not refresh assigned issue: {error}"))?
        .ok_or_else(|| format!("assigned issue {number} disappeared from {}", claim.repo))?;
    let artifact = match Classifier::new(workflow).classify_issue(&issue) {
        Ok(artifact) => artifact,
        Err(error) => {
            return quarantine_invalid_assignment(
                forge,
                policy,
                &claim.repo,
                claim.target,
                &claim.assignment,
                &format!("assigned issue is ambiguous: {error}"),
            )
            .await;
        }
    };
    let dependency_status =
        temper_workflow::dependency_state::status_for_artifact(forge, &claim.repo, &artifact).await;
    let dependencies_unresolved = artifact
        .relations
        .iter()
        .filter(|relation| relation.kind == RelationKind::Dependency)
        .any(|relation| !dependency_status.is_landed(&relation.target));
    LeaseManager::new(forge, policy)
        .converge_issue_assignment(
            &claim.repo,
            claim.target,
            &claim.assignment,
            &claim.queue_labels,
            &claim.claim_labels,
            dependencies_unresolved,
        )
        .await
        .map_err(|error| format!("could not converge abandoned issue assignment: {error}"))
}

pub async fn converge_startup_orphans(
    forge: &dyn Forge,
    policy: LeasePolicy,
    workflow: &ValidatedWorkflow,
    recovered: &BTreeMap<String, RecoveredClaim>,
    orphaned: &[temper_engine::RecoveredJob],
) -> Result<(), String> {
    for orphan in orphaned {
        let claim = recovered.get(&orphan.job_id).ok_or_else(|| {
            format!(
                "startup recovery lost durable context for {}",
                orphan.job_id
            )
        })?;
        converge_startup_claim(forge, policy, workflow, claim)
            .await
            .map_err(|error| {
                format!(
                    "could not converge orphaned claim {}: {error}",
                    orphan.job_id
                )
            })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
