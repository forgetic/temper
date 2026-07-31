use std::time::{Duration, Instant};

use temper_forge_forgejo::ForgejoForge;
use temper_forge_model::{
    CiJobQuery, CiJobStatus, Issue, ItemNumber, PullRequest, PullRequestState, RepositoryId,
};

use super::super::FinalStateEvidence;
use super::super::process::{ChildGuard, engine_block_on};
use super::{
    ASSERT_POLL, CompletedCiObservation, ci_job_evidence, ci_observation_evidence,
    drive_basic_delivery_to_open, implementation_pr, issue_evidence, poll_until, pr_evidence,
    require_labels, verify_engineer_pr,
};

/// Waits for one implementation PR to reach a complete provider CI snapshot
/// without requiring that snapshot to be landable. This is the generic live
/// strategy for contracts that need to inspect conservative terminal outcomes
/// (for example, interrupted or ambiguous provider failures) while the PR stays
/// at its exact tested head.
pub(in crate::live_manifest) fn drive_implementation_pr_terminal_ci_convergence(
    forge: &ForgejoForge,
    repository: &RepositoryId,
    issue: ItemNumber,
    admin_user: &str,
    standalone: &mut ChildGuard,
    timeout: Duration,
) -> Result<FinalStateEvidence, String> {
    let deadline = Instant::now() + timeout;
    drive_basic_delivery_to_open(forge, repository, issue, admin_user, standalone, deadline)?;

    let first = poll_until(deadline, standalone, || {
        engine_block_on(assert_terminal_ci(forge, repository, issue))
    })?;
    // Keep the observations independent and give the provider a poll boundary
    // across which an unstable attempt or task identity would be visible.
    std::thread::sleep(ASSERT_POLL);
    let second = poll_until(deadline, standalone, || {
        engine_block_on(assert_terminal_ci(forge, repository, issue))
    })?;

    let TerminalCiState {
        issue,
        pull_request,
        observation: second_observation,
    } = second;
    Ok(FinalStateEvidence {
        issue: issue_evidence(&issue),
        pull_request: pr_evidence(&pull_request),
        ci_jobs: second_observation
            .jobs
            .iter()
            .map(ci_job_evidence)
            .collect(),
        ci_observations: vec![
            ci_observation_evidence(&first.observation),
            ci_observation_evidence(&second_observation),
        ],
    })
}

struct TerminalCiState {
    issue: Issue,
    pull_request: PullRequest,
    observation: CompletedCiObservation,
}

async fn assert_terminal_ci(
    forge: &ForgejoForge,
    repository: &RepositoryId,
    issue: ItemNumber,
) -> Result<TerminalCiState, String> {
    let pull_request = implementation_pr(forge, repository, issue).await?;
    verify_engineer_pr(&pull_request, issue)?;
    if pull_request.state != PullRequestState::Open {
        return Err(format!(
            "implementation PR #{} did not remain open at terminal CI (state {:?})",
            pull_request.number, pull_request.state
        ));
    }
    require_labels(&pull_request.labels, &["implementation", "landing"])?;

    let listing = forge
        .list_ci_jobs_with_presence(
            repository,
            CiJobQuery {
                pull_request_id: Some(pull_request.id.clone()),
                ..CiJobQuery::default()
            },
        )
        .await
        .map_err(|error| format!("list_ci_jobs failed: {error}"))?;
    let matching_provider_run = listing.matching_ci_present();
    let mut jobs = listing.into_jobs();
    jobs.sort_by(|left, right| {
        left.created_at
            .cmp(&right.created_at)
            .then(left.id.cmp(&right.id))
    });
    if !matching_provider_run {
        return Err(format!(
            "no matching provider run for implementation PR #{}",
            pull_request.number
        ));
    }
    if jobs.is_empty() {
        return Err(format!(
            "matching provider run for implementation PR #{} has no materialized jobs",
            pull_request.number
        ));
    }
    if jobs.iter().any(|job| job.status != CiJobStatus::Completed) {
        return Err(format!(
            "implementation PR #{} CI jobs are not all terminal yet: {:?}",
            pull_request.number, jobs
        ));
    }

    let issue = forge
        .get_issue_by_number(repository, issue)
        .await
        .map_err(|error| format!("source issue lookup failed: {error}"))?
        .ok_or("source issue disappeared")?;
    Ok(TerminalCiState {
        issue,
        pull_request,
        observation: CompletedCiObservation {
            matching_provider_run,
            jobs,
        },
    })
}
