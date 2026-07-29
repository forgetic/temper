// SPDX-License-Identifier: MPL-2.0

use crate::support::*;
use temper_workflow::{ProviderRecovery, ProviderRecoveryDisposition, ProviderRecoveryFacts};

fn recovery_marker(number: u32) -> ProviderRecovery {
    ProviderRecovery {
        workstream_id: format!("provider-recovery-{number}"),
        failure_epoch: 1,
        disposition: ProviderRecoveryDisposition::Unknown,
        facts: ProviderRecoveryFacts {
            provider: "fixture-provider".to_string(),
            model: "fixture-model".to_string(),
            category: "redacted_unknown".to_string(),
            boundary: "sse".to_string(),
            event_kind: "stream_error".to_string(),
            status_present: false,
            code_present: false,
            http_status: None,
            provider_request_id: None,
            provider_error_code: None,
        },
        cumulative_failure_count: 2,
        deferral_count: 1,
        deferral_limit: 3,
        generation: 1,
        not_before: chrono::DateTime::from_timestamp_millis(2_000).unwrap(),
        epoch_started_at: chrono::DateTime::from_timestamp_millis(1_000).unwrap(),
        elapsed_ms: 100,
        slo_deadline: chrono::DateTime::from_timestamp_millis(10_000).unwrap(),
        idempotency_key: "a".repeat(64),
        source_attempt_id: "attempt-source".to_string(),
        due_assignment_attempt_id: None,
        health_event_id: None,
    }
}

struct StaleSuccessApplier;

#[async_trait::async_trait]
impl ResultApplier for StaleSuccessApplier {
    async fn apply(&self, _job: InFlightJob, _result: JobResult) -> temper_engine::ApplyOutcome {
        temper_engine::ApplyOutcome::Stale
    }
}

#[test]
fn stale_success_releases_due_assignment_without_clearing_recovery() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let root = MemoryForge::new();
        let repo = new_repo(&root, "stable").await;
        let issue_number = create_ready_issue(&root, &repo).await;
        let issue = root
            .get_issue_by_number(&repo, issue_number)
            .await
            .unwrap()
            .unwrap();
        root.update_issue(
            &issue.id,
            UpdateIssue {
                body: Some(render_metadata_block(&WorkflowMetadata {
                    kind: Some(ArtifactKindId::new("code")),
                    provider_recovery: Some(Box::new(recovery_marker(
                        u32::try_from(issue_number.get()).unwrap(),
                    ))),
                    ..WorkflowMetadata::default()
                })),
                ..UpdateIssue::default()
            },
        )
        .await
        .unwrap();

        let forge = Arc::new(root.as_user(role_user("engineer")));
        let lease = LeaseApplier::new(
            forge,
            policy(),
            "daemon-stale-success",
            Arc::new(StaleSuccessApplier),
            Arc::new(|| chrono::DateTime::from_timestamp_millis(2_000).unwrap()),
        );
        let mut job = open_pr_in_flight_job("acme/service", issue_number);
        let mut context: JobContext = serde_json::from_value(job.job_payload.clone()).unwrap();
        context.workspace = Some(WorkspaceManifest {
            coordination_key: format!("provider-recovery-{}", issue_number.get()),
            repos: vec![writable_repo("acme/service", "agent/provider-recovery")],
        });
        let mut wrong_job = job.clone();
        let mut wrong_context = context.clone();
        wrong_context.workspace.as_mut().unwrap().coordination_key =
            "unrelated-workstream".to_string();
        wrong_job.job_payload = serde_json::to_value(wrong_context).unwrap();
        let claim = temper_engine::ClaimContext {
            worker_id: "worker-stale-success".to_string(),
            daemon_boot_id: "daemon-stale-success".to_string(),
        };
        assert!(matches!(
            lease.claim(wrong_job, claim.clone()).await,
            temper_engine::ClaimOutcome::Retryable { .. }
        ));

        job.job_payload = serde_json::to_value(context).unwrap();
        assert_eq!(
            lease.claim(job.clone(), claim).await,
            temper_engine::ClaimOutcome::Claimed
        );
        let mut success = success_result(
            "worker-stale-success",
            &job.job_id,
            "acme/service",
            "agent/provider-recovery",
            "stale",
        );
        success.attempt_id = job.attempt_id.clone();
        assert_eq!(
            lease.apply(job, success).await,
            temper_engine::ApplyOutcome::Stale
        );

        let issue = root
            .get_issue_by_number(&repo, issue_number)
            .await
            .unwrap()
            .unwrap();
        let metadata = parse_metadata_block(&issue.body).unwrap().unwrap();
        let marker = metadata
            .provider_recovery
            .expect("stale success retains provider recovery");
        assert!(marker.due_assignment_attempt_id.is_none());
        assert!(metadata.assignment.is_none() && metadata.lease.is_none());
    })
}

#[test]
fn expired_or_corrupt_provider_recovery_parks_with_one_actionable_audit() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let root = MemoryForge::new();
        let repo = new_repo(&root, "stable").await;
        let expired_number = create_ready_issue(&root, &repo).await;
        let corrupt_number = create_ready_issue(&root, &repo).await;
        for (number, corrupt) in [(expired_number, false), (corrupt_number, true)] {
            let issue = root
                .get_issue_by_number(&repo, number)
                .await
                .unwrap()
                .unwrap();
            let mut marker = recovery_marker(u32::try_from(number.get()).unwrap());
            if corrupt {
                marker.generation = 0;
            }
            root.update_issue(
                &issue.id,
                UpdateIssue {
                    body: Some(render_metadata_block(&WorkflowMetadata {
                        kind: Some(ArtifactKindId::new("code")),
                        provider_recovery: Some(Box::new(marker)),
                        ..WorkflowMetadata::default()
                    })),
                    ..UpdateIssue::default()
                },
            )
            .await
            .unwrap();
        }
        let forge = Arc::new(root.as_user(role_user("engineer")));
        let workflow = workflow();
        let daemon = Daemon::new(Arc::new(handle));
        assert_eq!(
            daemon
                .enqueue_scanned_role_work(
                    forge.as_ref(),
                    &repo,
                    &workflow,
                    &workflow.compile(),
                    chrono::DateTime::from_timestamp_millis(10_000).unwrap(),
                    &RoleId::new("engineer"),
                    RoleFeedMode::Normal,
                )
                .await
                .unwrap(),
            0
        );
        for (number, expected) in [
            (expired_number, "provider recovery SLO expired"),
            (corrupt_number, "provider recovery metadata is corrupt"),
        ] {
            let issue = root
                .get_issue_by_number(&repo, number)
                .await
                .unwrap()
                .unwrap();
            assert!(issue.labels.contains(&"needs-human".to_string()));
            assert!(!issue.labels.contains(&"ready".to_string()));
            assert!(issue.assignees.is_empty());
            let comments = root.list_issue_comments(&issue.id).await.unwrap();
            assert_eq!(comments.len(), 1);
            assert!(comments[0].body.contains(expected));
            assert!(comments[0].body.contains("Operator repair:"));
        }
    })
}

#[test]
fn deferred_provider_recovery_is_restart_safe_health_wake_fenced_and_cleared_by_due_success() {
    use std::sync::atomic::{AtomicI64, Ordering};

    use secrecy::SecretString;
    use temper_engine::{
        ClaimContext, ClaimOutcome, ProviderHealthSignal, ProviderHealthWakeError,
        ProviderHealthWakeOutcome, ProviderHealthWaker, provider_health_signature,
    };

    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let root = MemoryForge::new();
        let repo = new_repo(&root, "stable").await;
        let issue_number = create_ready_issue(&root, &repo).await;
        let issue = root
            .get_issue_by_number(&repo, issue_number)
            .await
            .unwrap()
            .unwrap();
        root.update_issue(
            &issue.id,
            UpdateIssue {
                add_assignees: vec![UserId::new("queue-watcher")],
                ..UpdateIssue::default()
            },
        )
        .await
        .unwrap();
        let forge = Arc::new(root.as_user(role_user("engineer")));
        let workflow = Arc::new(workflow());
        let now_ms = Arc::new(AtomicI64::new(1_000));
        let clock: temper_engine::WallClock = {
            let now_ms = Arc::clone(&now_ms);
            Arc::new(move || {
                chrono::DateTime::from_timestamp_millis(now_ms.load(Ordering::SeqCst)).unwrap()
            })
        };
        let inner = Arc::new(ForgeApplier::new(forge.clone(), workflow.clone()));
        let lease = LeaseApplier::new(
            forge.clone(),
            policy(),
            "daemon-before-restart",
            inner.clone(),
            clock.clone(),
        );
        let mut job = open_pr_in_flight_job("acme/service", issue_number);
        let mut context: JobContext = serde_json::from_value(job.job_payload.clone()).unwrap();
        let mut workspace_repo = writable_repo("acme/service", "agent/provider-recovery");
        workspace_repo.default_branch = "stable".to_string();
        workspace_repo.base_branch = "stable".to_string();
        context.workspace = Some(WorkspaceManifest {
            coordination_key: "provider-recovery-810".to_string(),
            repos: vec![workspace_repo],
        });
        job.job_payload = serde_json::to_value(context).unwrap();
        let claim = ClaimContext {
            worker_id: "worker-before-restart".to_string(),
            daemon_boot_id: "daemon-before-restart".to_string(),
        };
        assert_eq!(
            lease.claim(job.clone(), claim.clone()).await,
            ClaimOutcome::Claimed
        );

        let deferred = model_recovery_failure_result(
            "worker-before-restart",
            &job.job_id,
            SessionRecoveryActionV1::ProviderDeferred,
            1,
            2,
        );
        assert_eq!(
            lease.apply(job.clone(), deferred.clone()).await,
            temper_engine::ApplyOutcome::RetryReleased
        );
        assert_eq!(
            lease.apply(job.clone(), deferred).await,
            temper_engine::ApplyOutcome::Stale,
            "duplicate delivery cannot account or release the attempt twice"
        );
        let deferred_issue = root
            .get_issue_by_number(&repo, issue_number)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(deferred_issue.labels, vec!["code", "ready"]);
        assert_eq!(
            deferred_issue.assignees,
            vec![UserId::new("queue-watcher")],
            "deferral releases only the claim assignee"
        );
        let marker = parse_metadata_block(&deferred_issue.body)
            .unwrap()
            .unwrap()
            .provider_recovery
            .expect("provider deferral is durable before assignment release");
        assert_eq!(marker.workstream_id, "provider-recovery-810");
        assert_eq!(marker.cumulative_failure_count, 2);
        assert_eq!(marker.generation, 1);
        assert!(marker.facts.status_present);
        assert!(marker.facts.code_present);
        assert!(marker.due_assignment_attempt_id.is_none());
        assert!(!deferred_issue.labels.contains(&"needs-human".to_string()));

        let replacement = Daemon::new(Arc::new(handle.clone()));
        assert_eq!(
            replacement
                .enqueue_scanned_role_work(
                    forge.as_ref(),
                    &repo,
                    workflow.as_ref(),
                    &workflow.compile(),
                    chrono::DateTime::from_timestamp_millis(1_500).unwrap(),
                    &RoleId::new("engineer"),
                    RoleFeedMode::Normal,
                )
                .await
                .unwrap(),
            0,
            "replacement daemon suppresses a pre-due durable marker"
        );

        now_ms.store(1_500, Ordering::SeqCst);
        let signal = ProviderHealthSignal {
            workstream_id: "provider-recovery-810".to_string(),
            failure_epoch: 1,
            expected_generation: 1,
            event_id: "provider-healthy-1".to_string(),
        };
        let waker = ProviderHealthWaker::new(
            forge.clone(),
            SecretString::from("host-health-secret".to_string()),
            clock.clone(),
        );
        assert_eq!(
            waker
                .advance(
                    &repo,
                    ArtifactSource::Issue {
                        number: issue_number
                    },
                    &signal,
                    "bad"
                )
                .await,
            Err(ProviderHealthWakeError::InvalidSignature)
        );
        assert_eq!(
            waker
                .advance(
                    &repo,
                    ArtifactSource::Issue {
                        number: issue_number
                    },
                    &signal,
                    "💣"
                )
                .await,
            Err(ProviderHealthWakeError::InvalidSignature),
            "non-ASCII signatures fail closed without panicking"
        );
        let signature = provider_health_signature("host-health-secret", &signal);
        assert_eq!(
            waker
                .advance(
                    &repo,
                    ArtifactSource::Issue {
                        number: issue_number
                    },
                    &signal,
                    &signature,
                )
                .await
                .unwrap(),
            ProviderHealthWakeOutcome::Advanced
        );
        assert_eq!(
            waker
                .advance(
                    &repo,
                    ArtifactSource::Issue {
                        number: issue_number
                    },
                    &signal,
                    &signature,
                )
                .await
                .unwrap(),
            ProviderHealthWakeOutcome::Duplicate
        );
        let unrelated_signal = ProviderHealthSignal {
            workstream_id: "unrelated-workstream".to_string(),
            failure_epoch: signal.failure_epoch,
            expected_generation: signal.expected_generation,
            event_id: signal.event_id.clone(),
        };
        assert_eq!(
            waker
                .advance(
                    &repo,
                    ArtifactSource::Issue {
                        number: issue_number
                    },
                    &unrelated_signal,
                    &provider_health_signature("host-health-secret", &unrelated_signal),
                )
                .await
                .unwrap(),
            ProviderHealthWakeOutcome::Stale,
            "a duplicate event id cannot bypass workstream scope"
        );
        assert_eq!(
            replacement
                .enqueue_scanned_role_work(
                    forge.as_ref(),
                    &repo,
                    workflow.as_ref(),
                    &workflow.compile(),
                    chrono::DateTime::from_timestamp_millis(1_500).unwrap(),
                    &RoleId::new("engineer"),
                    RoleFeedMode::Wake,
                )
                .await
                .unwrap(),
            1,
            "health-advanced marker is rediscovered through an ordinary wake scan"
        );

        let due_lease = LeaseApplier::new(
            forge.clone(),
            policy(),
            "daemon-after-restart",
            inner.clone(),
            clock.clone(),
        );
        let mut due_job = job.clone();
        due_job.attempt_id = Some("attempt-due".to_string());
        let due_claim = ClaimContext {
            worker_id: "worker-after-restart".to_string(),
            daemon_boot_id: "daemon-after-restart".to_string(),
        };
        assert_eq!(
            due_lease.claim(due_job.clone(), due_claim).await,
            ClaimOutcome::Claimed
        );
        let stale_success = success_result(
            "worker-before-restart",
            &job.job_id,
            "acme/service",
            "agent/provider-recovery",
            "stale",
        );
        assert_eq!(
            inner.apply(job.clone(), stale_success).await,
            temper_engine::ApplyOutcome::Stale,
            "unrelated success cannot publish through a due marker"
        );

        let mut deferred_again = model_recovery_failure_result(
            "worker-after-restart",
            &due_job.job_id,
            SessionRecoveryActionV1::ProviderDeferred,
            1,
            3,
        );
        deferred_again.attempt_id = due_job.attempt_id.clone();
        let recovery = deferred_again
            .failure
            .as_mut()
            .unwrap()
            .session_recovery
            .as_mut()
            .unwrap();
        recovery.attempt_id = due_job.attempt_id.clone().unwrap();
        recovery.deferral_count = 2;
        recovery.deferral_generation = 2;
        recovery.epoch_elapsed_ms = 500;
        assert_eq!(
            due_lease
                .apply(due_job.clone(), deferred_again.clone())
                .await,
            temper_engine::ApplyOutcome::RetryReleased
        );
        assert_eq!(
            due_lease.apply(due_job, deferred_again).await,
            temper_engine::ApplyOutcome::Stale,
            "another exhausted due attempt is accounted exactly once"
        );
        let deferred_again = root
            .get_issue_by_number(&repo, issue_number)
            .await
            .unwrap()
            .unwrap();
        let marker = parse_metadata_block(&deferred_again.body)
            .unwrap()
            .unwrap()
            .provider_recovery
            .unwrap();
        assert_eq!(marker.deferral_count, 2);
        assert_eq!(marker.generation, 3);
        assert!(marker.due_assignment_attempt_id.is_none());

        now_ms.store(2_000, Ordering::SeqCst);
        let final_lease = LeaseApplier::new(forge.clone(), policy(), "daemon-final", inner, clock);
        let mut final_job = job;
        final_job.attempt_id = Some("attempt-final".to_string());
        assert_eq!(
            final_lease
                .claim(
                    final_job.clone(),
                    ClaimContext {
                        worker_id: "worker-final".to_string(),
                        daemon_boot_id: "daemon-final".to_string(),
                    },
                )
                .await,
            ClaimOutcome::Claimed
        );
        let mut success = success_result(
            "worker-final",
            &final_job.job_id,
            "acme/service",
            "agent/provider-recovery",
            "recovered",
        );
        success.attempt_id = final_job.attempt_id.clone();
        assert_eq!(
            final_lease.apply(final_job, success).await,
            temper_engine::ApplyOutcome::Applied
        );
        let recovered = root
            .get_issue_by_number(&repo, issue_number)
            .await
            .unwrap()
            .unwrap();
        let metadata = parse_metadata_block(&recovered.body).unwrap().unwrap();
        assert!(metadata.provider_recovery.is_none());
        assert!(metadata.assignment.is_none() && metadata.lease.is_none());
        let pulls = root
            .list_pull_requests(&repo, PullRequestQuery::default())
            .await
            .unwrap();
        assert_eq!(
            pulls.len(),
            1,
            "matching due success publishes exactly once"
        );
    })
}
