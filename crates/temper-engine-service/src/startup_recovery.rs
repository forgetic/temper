// SPDX-License-Identifier: MPL-2.0

//! Durable assignment inventory and fresh-state startup convergence.

use std::collections::BTreeMap;

use chrono::Utc;
use temper_engine::Daemon;
use temper_forge::{
    Forge, ForgeError, ForgeResult, IssueQuery, IssueState, PullRequest, PullRequestQuery,
    PullRequestState, RepositoryId,
};
use temper_workflow::{
    ArtifactSource, AssignmentConverger, AssignmentValidation, CompiledWorkflow, DurableAssignment,
    LeasePolicy, METADATA_BEGIN, ValidatedWorkflow, parse_metadata_block,
};

#[derive(Clone)]
pub struct RecoveredClaim {
    repo: RepositoryId,
    target: ArtifactSource,
    assignment: DurableAssignment,
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
    let converger = AssignmentConverger::new(workflow, forge, policy);
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
                converger
                    .quarantine_target(
                        &repo,
                        target,
                        &format!("malformed workflow metadata: {error}"),
                    )
                    .await
                    .map_err(|error| error.to_string())?;
                continue;
            }
        };
        let Some(assignment) = metadata.assignment else {
            continue;
        };
        let (resolved_kind, expires_at) = match converger
            .validate_current(&repo, target, &assignment)
            .await
            .map_err(|error| error.to_string())?
        {
            AssignmentValidation::Valid { kind, expires_at } => (kind, expires_at),
            AssignmentValidation::Stale | AssignmentValidation::Quarantined => continue,
        };
        let job_id = assignment
            .job_id
            .clone()
            .expect("validated assignment has a job id");
        let worker_id = assignment
            .worker_id
            .clone()
            .expect("validated assignment has a worker id");
        let prior_boot = assignment
            .daemon_boot_id
            .clone()
            .expect("validated assignment has a daemon boot id");
        let claim = RecoveredClaim {
            repo: repo.clone(),
            target,
            assignment: assignment.clone(),
        };
        if expires_at <= now {
            converger
                .converge(&repo, target, &assignment)
                .await
                .map_err(|error| format!("could not converge expired assignment: {error}"))?;
            continue;
        }
        let reconstructed = if let Some(service) = artifact_context.as_deref() {
            temper_engine::recovered_job_from_assignment_with_artifact_context(
                forge,
                &repo,
                target,
                &assignment,
                resolved_kind,
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
                resolved_kind,
                workflow,
                compiled,
            )
            .await
        };
        let job = match reconstructed {
            Ok(job) => job,
            Err(reason) => {
                tracing::warn!(job_id = %job_id, %reason, "quarantining impossible durable assignment");
                converger
                    .quarantine_current(&repo, target, &assignment, &reason)
                    .await
                    .map_err(|error| error.to_string())?;
                continue;
            }
        };
        let attempt_id = claim.assignment.attempt_id.clone();
        prepared.push((
            job_id,
            claim,
            temper_engine::RecoveredJob {
                job_id: job.job_id,
                attempt_id,
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
            converger
                .quarantine_current(
                    &claim.repo,
                    claim.target,
                    &claim.assignment,
                    "multiple durable assignments claim the same job id",
                )
                .await
                .map_err(|error| error.to_string())?;
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

pub async fn converge_startup_orphans(
    forge: &dyn Forge,
    policy: LeasePolicy,
    workflow: &ValidatedWorkflow,
    recovered: &BTreeMap<String, RecoveredClaim>,
    orphaned: &[temper_engine::RecoveredJob],
) -> Result<(), String> {
    let converger = AssignmentConverger::new(workflow, forge, policy);
    for orphan in orphaned {
        let claim = recovered.get(&orphan.job_id).ok_or_else(|| {
            format!(
                "startup recovery lost durable context for {}",
                orphan.job_id
            )
        })?;
        converger
            .converge(&claim.repo, claim.target, &claim.assignment)
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
