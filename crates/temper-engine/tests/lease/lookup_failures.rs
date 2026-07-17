// SPDX-License-Identifier: MPL-2.0

use std::sync::Mutex;

use chrono::{DateTime, Utc};
use temper_forge_memory::FaultOp;
use temper_workflow::WorkflowMetadata;

use super::*;

fn claim_context() -> temper_engine::ClaimContext {
    temper_engine::ClaimContext {
        worker_id: "worker-a".to_string(),
        daemon_boot_id: "daemon-1".to_string(),
    }
}

fn recording_inner() -> (
    Arc<RecordingApplier>,
    temper_engine_io::CqReceiver<(InFlightJob, JobResult)>,
) {
    let (tx, rx) = temper_engine_io::channel();
    (
        Arc::new(RecordingApplier {
            tx,
            forge: None,
            repo: None,
            issue: None,
            lease_tx: None,
            freshness_response: None,
        }),
        rx,
    )
}

async fn issue_metadata(
    forge: &MemoryForge,
    repo: &RepositoryId,
    issue: ItemNumber,
) -> WorkflowMetadata {
    let issue = forge
        .get_issue_by_number(repo, issue)
        .await
        .expect("issue reload succeeds")
        .expect("issue exists");
    parse_metadata_block(&issue.body)
        .expect("issue metadata parses")
        .unwrap_or_default()
}

fn controlled_clock(
    initial: DateTime<Utc>,
) -> (Arc<Mutex<DateTime<Utc>>>, temper_engine::WallClock) {
    let now = Arc::new(Mutex::new(initial));
    let captured = Arc::clone(&now);
    (now, Arc::new(move || *captured.lock().expect("clock lock")))
}

#[test]
fn repository_lookup_failure_is_retryable_while_missing_and_malformed_targets_are_stale() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge).await;
        let issue = create_ready_issue(&forge, &repo).await;
        let (inner, mut rx) = recording_inner();
        let applier = LeaseApplier::new(
            forge.clone(),
            policy(),
            "daemon-1",
            inner,
            temper_engine::system_clock(),
        );

        forge.fail_next(
            FaultOp::GetRepositoryByPath,
            "simulated claim repository lookup failure",
        );
        let outcome = applier.claim(in_flight_job(issue), claim_context()).await;
        assert!(matches!(
            outcome,
            temper_engine::ClaimOutcome::Retryable { reason }
                if reason.contains("simulated claim repository lookup failure")
        ));
        assert!(rx.try_recv().is_none());
        let metadata = issue_metadata(&forge, &repo, issue).await;
        assert!(metadata.assignment.is_none());
        assert!(metadata.lease.is_none());

        let mut missing = in_flight_job(issue);
        missing.repo = "acme/missing".to_string();
        assert!(matches!(
            applier.claim(missing, claim_context()).await,
            temper_engine::ClaimOutcome::Stale { .. }
        ));

        let mut malformed = in_flight_job(issue);
        malformed.repo = "not-a-repository-path".to_string();
        assert!(matches!(
            applier.claim(malformed, claim_context()).await,
            temper_engine::ClaimOutcome::Stale { .. }
        ));

        let mut malformed_artifact = in_flight_job(issue);
        malformed_artifact.artifact.item = json!("not-a-number");
        assert!(matches!(
            applier.claim(malformed_artifact, claim_context()).await,
            temper_engine::ClaimOutcome::Stale { .. }
        ));
    })
}

#[test]
fn apply_lookup_failure_is_retryable_and_preserves_claim_for_one_recovery_apply() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge).await;
        let issue = create_ready_issue(&forge, &repo).await;
        let (inner, mut rx) = recording_inner();
        let applier = LeaseApplier::new(
            forge.clone(),
            policy(),
            "daemon-1",
            inner,
            temper_engine::system_clock(),
        );
        let job = in_flight_job(issue);
        let result = job_result(&job.job_id);
        assert_eq!(
            applier.claim(job.clone(), claim_context()).await,
            temper_engine::ClaimOutcome::Claimed
        );

        forge.fail_next(
            FaultOp::GetRepositoryByPath,
            "simulated apply repository lookup failure",
        );
        let failed = applier.apply(job.clone(), result.clone()).await;
        assert!(matches!(
            failed,
            temper_engine::ApplyOutcome::Retryable { reason }
                if reason.contains("simulated apply repository lookup failure")
        ));
        assert!(rx.try_recv().is_none());
        let metadata = issue_metadata(&forge, &repo, issue).await;
        assert!(metadata.assignment.is_some());
        assert!(metadata.lease.is_some());

        assert_eq!(
            applier.apply(job.clone(), result.clone()).await,
            temper_engine::ApplyOutcome::Applied
        );
        assert_eq!(rx.recv().await, Some((job, result)));
        assert!(rx.try_recv().is_none());
        let metadata = issue_metadata(&forge, &repo, issue).await;
        assert!(metadata.assignment.is_none());
        assert!(metadata.lease.is_none());
    })
}

#[test]
fn heartbeat_lookup_failure_retains_context_and_later_advances_the_lease() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge).await;
        let issue = create_ready_issue(&forge, &repo).await;
        let (inner, mut rx) = recording_inner();
        let initial = "2026-07-15T14:50:00Z"
            .parse::<DateTime<Utc>>()
            .expect("valid initial time");
        let (now, clock) = controlled_clock(initial);
        let applier = LeaseApplier::new(forge.clone(), policy(), "daemon-1", inner, clock);
        let job = in_flight_job(issue);
        let context = claim_context();
        assert_eq!(
            applier.claim(job.clone(), context.clone()).await,
            temper_engine::ClaimOutcome::Claimed
        );
        let initial_metadata = issue_metadata(&forge, &repo, issue).await;

        *now.lock().expect("clock lock") = initial + chrono::Duration::seconds(60);
        forge.fail_next(
            FaultOp::GetRepositoryByPath,
            "simulated heartbeat repository lookup failure",
        );
        applier.heartbeat(job.clone(), context.clone()).await;
        let failed_metadata = issue_metadata(&forge, &repo, issue).await;
        assert_eq!(failed_metadata.assignment, initial_metadata.assignment);
        assert_eq!(failed_metadata.lease, initial_metadata.lease);

        forge.fail_next(
            FaultOp::GetRepositoryByPath,
            "probe retained process-local heartbeat context",
        );
        assert!(matches!(
            applier.apply(job.clone(), job_result(&job.job_id)).await,
            temper_engine::ApplyOutcome::Retryable { reason }
                if reason.contains("probe retained process-local heartbeat context")
        ));
        assert!(rx.try_recv().is_none());

        *now.lock().expect("clock lock") = initial + chrono::Duration::seconds(120);
        applier.heartbeat(job, context).await;
        let refreshed = issue_metadata(&forge, &repo, issue).await;
        assert!(
            refreshed
                .lease
                .as_ref()
                .expect("refreshed lease")
                .expires_at
                > initial_metadata
                    .lease
                    .as_ref()
                    .expect("initial lease")
                    .expires_at
        );
        assert!(
            refreshed
                .assignment
                .as_ref()
                .expect("refreshed assignment")
                .expires_at
                > initial_metadata
                    .assignment
                    .as_ref()
                    .expect("initial assignment")
                    .expires_at
        );
    })
}

#[test]
fn release_lookup_failure_drops_local_context_but_preserves_durable_assignment() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge).await;
        let issue = create_ready_issue(&forge, &repo).await;
        let (inner, mut rx) = recording_inner();
        let applier = LeaseApplier::new(
            forge.clone(),
            policy(),
            "daemon-1",
            inner,
            temper_engine::system_clock(),
        );
        let job = in_flight_job(issue);
        let context = claim_context();
        assert_eq!(
            applier.claim(job.clone(), context.clone()).await,
            temper_engine::ClaimOutcome::Claimed
        );
        let claimed = issue_metadata(&forge, &repo, issue).await;

        forge.fail_next(
            FaultOp::GetRepositoryByPath,
            "simulated release repository lookup failure",
        );
        applier.release_claim(job.clone(), context).await;
        let after_release = issue_metadata(&forge, &repo, issue).await;
        assert_eq!(after_release.assignment, claimed.assignment);
        assert_eq!(after_release.lease, claimed.lease);

        forge.fail_next(
            FaultOp::GetRepositoryByPath,
            "must remain unconsumed when local context was removed",
        );
        assert_eq!(
            applier.apply(job.clone(), job_result(&job.job_id)).await,
            temper_engine::ApplyOutcome::Stale
        );
        assert!(rx.try_recv().is_none());
    })
}

#[test]
fn recovered_apply_lookup_failure_retries_then_applies_once() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge).await;
        let issue = create_ready_issue(&forge, &repo).await;
        let job = in_flight_job(issue);
        let result = job_result(&job.job_id);
        let context = claim_context();
        let (initial_inner, _initial_rx) = recording_inner();
        let initial = LeaseApplier::new(
            forge.clone(),
            policy(),
            "daemon-1",
            initial_inner,
            temper_engine::system_clock(),
        );
        assert_eq!(
            initial.claim(job.clone(), context.clone()).await,
            temper_engine::ClaimOutcome::Claimed
        );
        drop(initial);

        let (recovered_inner, mut rx) = recording_inner();
        let recovered = LeaseApplier::new(
            forge.clone(),
            policy(),
            "daemon-2",
            recovered_inner,
            temper_engine::system_clock(),
        );
        forge.fail_next(
            FaultOp::GetRepositoryByPath,
            "simulated recovered apply repository lookup failure",
        );
        let failed = recovered
            .apply_recovered(job.clone(), result.clone(), context.clone())
            .await;
        assert!(matches!(
            failed,
            temper_engine::ApplyOutcome::Retryable { reason }
                if reason.contains("simulated recovered apply repository lookup failure")
        ));
        assert!(rx.try_recv().is_none());
        let retained = issue_metadata(&forge, &repo, issue).await;
        assert!(retained.assignment.is_some());
        assert!(retained.lease.is_some());

        assert_eq!(
            recovered
                .apply_recovered(job.clone(), result.clone(), context)
                .await,
            temper_engine::ApplyOutcome::Applied
        );
        assert_eq!(rx.recv().await, Some((job, result)));
        assert!(rx.try_recv().is_none());
    })
}
