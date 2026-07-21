// SPDX-License-Identifier: MPL-2.0

use super::*;
use temper_forge::UpdateIssue;
use temper_workflow::{
    RecoveredHeartbeatOutcome, RecoveredOwnershipLossReason, WorkflowMetadata,
    replace_metadata_block,
};

async fn rewrite_issue_metadata(
    forge: &MemoryForge,
    repo: &RepositoryId,
    number: ItemNumber,
    mutate: impl FnOnce(&mut WorkflowMetadata),
) {
    let issue = forge
        .get_issue_by_number(repo, number)
        .await
        .expect("issue reload succeeds")
        .expect("issue exists");
    let mut metadata = parse_metadata_block(&issue.body)
        .expect("issue metadata parses")
        .expect("issue metadata exists");
    mutate(&mut metadata);
    let body = replace_metadata_block(&issue.body, &metadata).expect("metadata renders");
    forge
        .update_issue(
            &issue.id,
            UpdateIssue {
                body: Some(body),
                ..UpdateIssue::default()
            },
        )
        .await
        .expect("metadata rewrite succeeds");
}

#[test]
fn definitive_heartbeat_loss_is_monotonic_and_scoped_to_exact_attempt() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge).await;
        let issue = create_ready_issue(&forge, &repo).await;
        let (tx, mut rx) = temper_engine_io::channel();
        let applier = LeaseApplier::new(
            forge.clone(),
            policy(),
            "daemon-1",
            Arc::new(RecordingApplier {
                tx,
                forge: None,
                repo: None,
                issue: None,
                lease_tx: None,
                freshness_response: None,
            }),
            temper_engine::system_clock(),
        );
        let job = in_flight_job(issue);
        let context = temper_engine::ClaimContext {
            worker_id: "worker-a".to_string(),
            daemon_boot_id: "daemon-1".to_string(),
        };
        assert_eq!(
            applier.claim(job.clone(), context.clone()).await,
            temper_engine::ClaimOutcome::Claimed
        );
        assert_eq!(
            applier.heartbeat(job.clone(), context.clone()).await,
            RecoveredHeartbeatOutcome::Owned
        );
        let claimed_issue = forge
            .get_issue_by_number(&repo, issue)
            .await
            .unwrap()
            .unwrap();
        let claimed = parse_metadata_block(&claimed_issue.body).unwrap().unwrap();

        rewrite_issue_metadata(&forge, &repo, issue, |metadata| metadata.assignment = None).await;
        assert_eq!(
            applier.heartbeat(job.clone(), context.clone()).await,
            RecoveredHeartbeatOutcome::OwnershipLost {
                reason: RecoveredOwnershipLossReason::AssignmentAbsent,
            }
        );

        let restored_assignment = claimed.assignment.clone();
        let restored_lease = claimed.lease.clone();
        rewrite_issue_metadata(&forge, &repo, issue, move |metadata| {
            metadata.assignment = restored_assignment;
            metadata.lease = restored_lease;
        })
        .await;
        assert!(matches!(
            applier.heartbeat(job.clone(), context.clone()).await,
            RecoveredHeartbeatOutcome::OwnershipLost { .. }
        ));
        assert_eq!(
            applier.apply(job.clone(), job_result(&job.job_id)).await,
            temper_engine::ApplyOutcome::Stale,
            "restoring durable metadata cannot restore revoked local authority"
        );
        assert!(rx.try_recv().is_none());

        let mut newer_job = job.clone();
        newer_job.attempt_id = Some("attempt-newer".to_string());
        rewrite_issue_metadata(&forge, &repo, issue, |metadata| {
            metadata
                .assignment
                .as_mut()
                .expect("restored assignment")
                .attempt_id = newer_job.attempt_id.clone();
        })
        .await;
        assert_eq!(
            applier.heartbeat(newer_job.clone(), context.clone()).await,
            RecoveredHeartbeatOutcome::Owned,
            "a revoked older attempt does not revoke a different exact attempt"
        );
        let mut newer_result = job_result(&newer_job.job_id);
        newer_result.attempt_id = newer_job.attempt_id.clone();
        assert_eq!(
            applier.apply(newer_job.clone(), newer_result.clone()).await,
            temper_engine::ApplyOutcome::Applied
        );
        assert_eq!(rx.recv().await, Some((newer_job, newer_result)));
    })
}

#[test]
fn recovered_apply_is_stale_for_definitive_lease_loss() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge).await;
        let issue = create_ready_issue(&forge, &repo).await;
        let job = in_flight_job(issue);
        let result = job_result(&job.job_id);
        let context = temper_engine::ClaimContext {
            worker_id: result.worker_id.clone(),
            daemon_boot_id: "daemon-1".to_string(),
        };
        let (initial_tx, _initial_rx) = temper_engine_io::channel();
        let initial = LeaseApplier::new(
            forge.clone(),
            policy(),
            "daemon-1",
            Arc::new(RecordingApplier {
                tx: initial_tx,
                forge: None,
                repo: None,
                issue: None,
                lease_tx: None,
                freshness_response: None,
            }),
            temper_engine::system_clock(),
        );
        assert_eq!(
            initial.claim(job.clone(), context.clone()).await,
            temper_engine::ClaimOutcome::Claimed
        );
        drop(initial);
        rewrite_issue_metadata(&forge, &repo, issue, |metadata| metadata.lease = None).await;

        let (tx, mut rx) = temper_engine_io::channel();
        let recovered = LeaseApplier::new(
            forge,
            policy(),
            "daemon-2",
            Arc::new(RecordingApplier {
                tx,
                forge: None,
                repo: None,
                issue: None,
                lease_tx: None,
                freshness_response: None,
            }),
            temper_engine::system_clock(),
        );
        assert_eq!(
            recovered.apply_recovered(job, result, context).await,
            temper_engine::ApplyOutcome::Stale
        );
        assert!(rx.try_recv().is_none());
    })
}
