// SPDX-License-Identifier: MPL-2.0

use std::time::{Duration, Instant};

use temper_forge_forgejo::ForgejoForge;
use temper_forge_model::{
    CiJobConclusion, CiJobQuery, CiJobStatus, ItemNumber, PullRequestId, RepositoryId,
};

use super::super::process::{ChildGuard, engine_block_on};
use super::super::{CiHeadEvidence, FinalStateEvidence};
use super::{
    ASSERT_POLL, CompletedCiObservation, ci_job_evidence, ci_observation_evidence,
    drive_basic_delivery_to_open, implementation_pr, poll_until,
};

/// Drives one real failed exact head through the dedicated CI monitor and then
/// waits for the same PR to land at a distinct passing repair head.
pub(in crate::live_manifest) fn drive_ci_poll_exact_head_repair_convergence(
    forge: &ForgejoForge,
    repository: &RepositoryId,
    issue: ItemNumber,
    admin_user: &str,
    standalone: &mut ChildGuard,
    timeout: Duration,
) -> Result<FinalStateEvidence, String> {
    let started = Instant::now();
    let deadline = started + timeout;
    drive_basic_delivery_to_open(forge, repository, issue, admin_user, standalone, deadline)?;

    let pull = engine_block_on(implementation_pr(forge, repository, issue))?;
    let initial_head = pull
        .head_sha
        .clone()
        .filter(|head| !head.trim().is_empty())
        .ok_or("initial implementation PR has no exact head")?;
    let pull_id = pull.id.clone();
    let first_initial = poll_until(deadline, standalone, || {
        engine_block_on(exact_head_observation(
            forge,
            repository,
            &pull_id,
            &initial_head,
            CiJobConclusion::Failure,
            true,
        ))
    })?;
    // Keep both retained snapshots independent so an unstable provider
    // run/job/attempt identity cannot pass the provenance assertion.
    std::thread::sleep(ASSERT_POLL);
    let second_initial = poll_until(deadline, standalone, || {
        engine_block_on(exact_head_observation(
            forge,
            repository,
            &pull_id,
            &initial_head,
            CiJobConclusion::Failure,
            true,
        ))
    })?;
    let initial_observed_after_ms = duration_ms(started.elapsed());

    let mut final_state = poll_until(deadline, standalone, || {
        engine_block_on(super::assert_converged(
            forge, repository, issue, admin_user,
        ))
    })?;
    let repaired_head = final_state
        .pull_request
        .head_sha
        .clone()
        .filter(|head| !head.trim().is_empty())
        .ok_or("repaired implementation PR has no exact head")?;
    if repaired_head == initial_head {
        return Err(format!(
            "engineer repair did not advance the implementation PR head `{initial_head}`"
        ));
    }
    // A pull-request-only query can legitimately retain completed jobs from
    // both historical heads. Read the repaired commit explicitly so stale red
    // evidence is retained for the initial phase but cannot influence the
    // repaired phase or its passing assertion.
    let first_repaired = poll_until(deadline, standalone, || {
        engine_block_on(exact_head_observation(
            forge,
            repository,
            &pull_id,
            &repaired_head,
            CiJobConclusion::Success,
            false,
        ))
    })?;
    std::thread::sleep(ASSERT_POLL);
    let second_repaired = poll_until(deadline, standalone, || {
        engine_block_on(exact_head_observation(
            forge,
            repository,
            &pull_id,
            &repaired_head,
            CiJobConclusion::Success,
            false,
        ))
    })?;
    if second_repaired.jobs.iter().any(|job| {
        job.commit_sha != repaired_head
            || job.verified_failure.is_some()
            || job.conclusion != Some(CiJobConclusion::Success)
    }) {
        return Err(format!(
            "repaired head `{repaired_head}` retained stale or non-passing CI evidence: {:?}",
            second_repaired.jobs
        ));
    }

    final_state.ci_jobs = second_repaired.jobs.iter().map(ci_job_evidence).collect();
    final_state.ci_observations = vec![
        ci_observation_evidence(&first_repaired),
        ci_observation_evidence(&second_repaired),
    ];
    final_state.ci_heads = vec![
        CiHeadEvidence {
            phase: "initial".to_string(),
            head_sha: initial_head,
            observed_after_ms: initial_observed_after_ms,
            jobs: second_initial.jobs.iter().map(ci_job_evidence).collect(),
            observations: vec![
                ci_observation_evidence(&first_initial),
                ci_observation_evidence(&second_initial),
            ],
        },
        CiHeadEvidence {
            phase: "repaired".to_string(),
            head_sha: repaired_head,
            observed_after_ms: duration_ms(started.elapsed()),
            jobs: final_state.ci_jobs.clone(),
            observations: final_state.ci_observations.clone(),
        },
    ];
    Ok(final_state)
}

async fn exact_head_observation(
    forge: &ForgejoForge,
    repository: &RepositoryId,
    pull_request_id: &PullRequestId,
    head: &str,
    expected: CiJobConclusion,
    require_proof: bool,
) -> Result<CompletedCiObservation, String> {
    let listing = forge
        .list_ci_jobs_with_presence(
            repository,
            CiJobQuery {
                pull_request_id: Some(pull_request_id.clone()),
                commit_sha: Some(head.to_string()),
                ..CiJobQuery::default()
            },
        )
        .await
        .map_err(|error| format!("read exact-head CI for `{head}`: {error}"))?;
    if !listing.matching_ci_present() {
        return Err(format!("no matching provider run for exact head `{head}`"));
    }
    let mut jobs = listing.into_jobs();
    jobs.sort_by(|left, right| {
        left.created_at
            .cmp(&right.created_at)
            .then(left.id.cmp(&right.id))
    });
    if jobs.len() != 1 {
        return Err(format!(
            "expected one ordinary-source-check job for exact head `{head}`, observed {}",
            jobs.len()
        ));
    }
    let job = &jobs[0];
    if job.name != "ordinary-source-check"
        || job.status != CiJobStatus::Completed
        || job.conclusion != Some(expected)
        || job.commit_sha != head
    {
        return Err(format!(
            "exact head `{head}` has not reached expected {expected:?} ordinary source CI: {job:?}"
        ));
    }
    if require_proof && job.verified_failure.is_none() {
        return Err(format!(
            "exact head `{head}` failure has no verified ordinary-failure proof"
        ));
    }
    Ok(CompletedCiObservation {
        matching_provider_run: true,
        jobs,
    })
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
