use super::*;

#[test]
fn no_configured_diagnostic_parks_once_after_unsupported_retry() {
    temper_engine_io::block_on(async {
        let fixture = Fixture::new().await;
        let workflow = workflow_without_diagnostic();
        let compiled = workflow.compile();
        let recover = || {
            recover_interrupted_ci(
                &fixture.forge,
                &fixture.repository,
                &workflow,
                &compiled,
                fixture.now,
                ArtifactAddress::pull_request(fixture.pull_request.number),
            )
        };

        assert_eq!(recover().await, InterruptedCiRecoveryOutcome::Waiting);
        assert_eq!(recover().await, InterruptedCiRecoveryOutcome::Parked);
        assert_eq!(fixture.forge.ci_retry_requests().len(), 1);
        assert!(requires_human_attention(&fixture.fresh().await.labels));
        assert_eq!(
            fixture
                .forge
                .list_pull_request_comments(&fixture.pull_request.id)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(recover().await, InterruptedCiRecoveryOutcome::Suppressed);
    });
}

#[test]
fn unpublished_diagnostic_rollback_reopens_the_same_publication_boundary() {
    temper_engine_io::block_on(async {
        let fixture = Fixture::new().await;
        assert_eq!(
            fixture.recover().await,
            InterruptedCiRecoveryOutcome::Waiting
        );
        assert_eq!(
            fixture.recover().await,
            InterruptedCiRecoveryOutcome::DispatchDiagnostic
        );
        let manager = LeaseManager::new(&fixture.forge, LeasePolicy::new(Duration::minutes(30)));
        let expected = diagnostic_assignment(&fixture);
        let claimed = manager
            .claim_assignment(
                &fixture.repository.id,
                ArtifactSource::PullRequest {
                    number: fixture.pull_request.number,
                },
                AssignmentClaimRequest {
                    assignment: expected,
                    mutation: AssignmentMutation::default(),
                },
                fixture.now,
            )
            .await
            .unwrap();
        manager
            .rollback_assignment(
                &fixture.repository.id,
                ArtifactSource::PullRequest {
                    number: fixture.pull_request.number,
                },
                &claimed,
            )
            .await
            .unwrap();

        let metadata = parse_metadata_block(&fixture.fresh().await.body)
            .unwrap()
            .unwrap();
        assert!(metadata.assignment.is_none() && metadata.lease.is_none());
        assert_eq!(
            metadata
                .interrupted_ci_recovery
                .unwrap()
                .diagnostic
                .unwrap()
                .job_id,
            None
        );
        assert_eq!(
            fixture.recover().await,
            InterruptedCiRecoveryOutcome::DispatchDiagnostic
        );
        assert_eq!(fixture.forge.ci_retry_requests().len(), 1);
    });
}

#[test]
fn published_diagnostic_fence_rejects_the_same_deterministic_job_reclaim() {
    temper_engine_io::block_on(async {
        let fixture = Fixture::new().await;
        assert_eq!(
            fixture.recover().await,
            InterruptedCiRecoveryOutcome::Waiting
        );
        assert_eq!(
            fixture.recover().await,
            InterruptedCiRecoveryOutcome::DispatchDiagnostic
        );
        let manager = LeaseManager::new(&fixture.forge, LeasePolicy::new(Duration::minutes(30)));
        let claimed = manager
            .claim_assignment(
                &fixture.repository.id,
                ArtifactSource::PullRequest {
                    number: fixture.pull_request.number,
                },
                AssignmentClaimRequest {
                    assignment: diagnostic_assignment(&fixture),
                    mutation: AssignmentMutation::default(),
                },
                fixture.now,
            )
            .await
            .unwrap();
        manager
            .release_assignment(
                &fixture.repository.id,
                ArtifactSource::PullRequest {
                    number: fixture.pull_request.number,
                },
                &claimed,
            )
            .await
            .unwrap();

        let reclaim = manager
            .claim_assignment(
                &fixture.repository.id,
                ArtifactSource::PullRequest {
                    number: fixture.pull_request.number,
                },
                AssignmentClaimRequest {
                    assignment: diagnostic_assignment(&fixture),
                    mutation: AssignmentMutation::default(),
                },
                fixture.now,
            )
            .await;
        assert!(matches!(
            reclaim,
            Err(LeaseError::AssignmentConflict { .. })
        ));
        let metadata = parse_metadata_block(&fixture.fresh().await.body)
            .unwrap()
            .unwrap();
        assert!(metadata.assignment.is_none() && metadata.lease.is_none());
        assert_eq!(
            metadata
                .interrupted_ci_recovery
                .unwrap()
                .diagnostic
                .unwrap()
                .job_id,
            claimed.job_id
        );
    });
}

#[test]
fn abandoned_diagnostic_convergence_keeps_the_publication_fence_and_parks() {
    temper_engine_io::block_on(async {
        let fixture = Fixture::new().await;
        assert_eq!(
            fixture.recover().await,
            InterruptedCiRecoveryOutcome::Waiting
        );
        assert_eq!(
            fixture.recover().await,
            InterruptedCiRecoveryOutcome::DispatchDiagnostic
        );
        let manager = LeaseManager::new(&fixture.forge, LeasePolicy::new(Duration::minutes(30)));
        let claimed = manager
            .claim_assignment(
                &fixture.repository.id,
                ArtifactSource::PullRequest {
                    number: fixture.pull_request.number,
                },
                AssignmentClaimRequest {
                    assignment: diagnostic_assignment(&fixture),
                    mutation: AssignmentMutation::default(),
                },
                fixture.now,
            )
            .await
            .unwrap();
        manager
            .rollback_assignment_snapshot(
                &fixture.repository.id,
                ArtifactSource::PullRequest {
                    number: fixture.pull_request.number,
                },
                &claimed,
            )
            .await
            .unwrap();

        let metadata = parse_metadata_block(&fixture.fresh().await.body)
            .unwrap()
            .unwrap();
        assert!(metadata.assignment.is_none() && metadata.lease.is_none());
        assert_eq!(
            metadata
                .interrupted_ci_recovery
                .unwrap()
                .diagnostic
                .unwrap()
                .job_id,
            claimed.job_id,
            "an assignment that may have been published must remain exhausted"
        );
        assert_eq!(
            fixture.recover().await,
            InterruptedCiRecoveryOutcome::Parked
        );
        assert!(requires_human_attention(&fixture.fresh().await.labels));
        assert_eq!(fixture.forge.ci_retry_requests().len(), 1);
    });
}

#[test]
fn audit_cleanup_conflict_reuses_the_published_comment_after_restart() {
    temper_engine_io::block_on(async {
        let fixture = Fixture::new().await;
        complete_diagnostic(&fixture).await;

        let pull_request = fixture.fresh().await;
        let mut metadata = parse_metadata_block(&pull_request.body).unwrap().unwrap();
        metadata
            .interrupted_ci_recovery
            .as_mut()
            .unwrap()
            .parking_barrier_installed = true;
        fixture
            .forge
            .update_pull_request(
                &pull_request.id,
                UpdatePullRequest {
                    body: Some(replace_metadata_block(&pull_request.body, &metadata).unwrap()),
                    add_labels: vec![NEEDS_HUMAN_LABEL.to_string()],
                    expected_version: Some(pull_request.version),
                    ..UpdatePullRequest::default()
                },
            )
            .await
            .unwrap();
        fixture.forge.conflict_next(
            FaultOp::UpdatePullRequest,
            "injected interrupted-CI marker cleanup conflict",
        );

        assert!(matches!(
            fixture.recover().await,
            InterruptedCiRecoveryOutcome::Retryable { reason }
                if reason.contains("parking_marker_clear_failed")
        ));
        assert_eq!(
            fixture
                .forge
                .list_pull_request_comments(&fixture.pull_request.id)
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(
            parse_metadata_block(&fixture.fresh().await.body)
                .unwrap()
                .unwrap()
                .interrupted_ci_recovery
                .is_some()
        );

        assert_eq!(
            fixture.recover().await,
            InterruptedCiRecoveryOutcome::Parked
        );
        assert_eq!(
            fixture
                .forge
                .list_pull_request_comments(&fixture.pull_request.id)
                .await
                .unwrap()
                .len(),
            1
        );
    });
}

#[test]
fn same_attempt_evidence_refresh_preserves_the_bounded_retry_progress() {
    temper_engine_io::block_on(async {
        let fixture = Fixture::new().await;
        assert_eq!(
            fixture.recover().await,
            InterruptedCiRecoveryOutcome::Waiting
        );
        assert_eq!(fixture.forge.ci_retry_requests().len(), 1);

        let mut refreshed = job(
            &fixture.repository,
            &fixture.pull_request,
            "591",
            "1",
            CiJobStatus::Completed,
            Some(CiJobConclusion::RunnerLost),
            fixture.now + Duration::seconds(30),
        );
        refreshed.provider_reason =
            Some("runner loss confirmed after delayed provider update".into());
        fixture
            .forge
            .seed_ci_jobs(&fixture.repository.id, vec![refreshed]);

        assert_eq!(
            fixture.recover().await,
            InterruptedCiRecoveryOutcome::Waiting,
            "the reordered evidence is committed before another action"
        );
        assert_eq!(fixture.forge.ci_retry_requests().len(), 1);
        assert_eq!(
            fixture.recover().await,
            InterruptedCiRecoveryOutcome::DispatchDiagnostic
        );
        assert_eq!(fixture.forge.ci_retry_requests().len(), 1);
        let state = parse_metadata_block(&fixture.fresh().await.body)
            .unwrap()
            .unwrap()
            .interrupted_ci_recovery
            .unwrap();
        assert_eq!(
            state.evidence[0].provider_reason.as_deref(),
            Some("runner loss confirmed after delayed provider update")
        );
    });
}

#[test]
fn unrelated_human_attention_cancels_recovery_without_removing_the_barrier() {
    temper_engine_io::block_on(async {
        let fixture = Fixture::new().await;
        assert_eq!(
            fixture.recover().await,
            InterruptedCiRecoveryOutcome::Waiting
        );
        let pull_request = fixture.fresh().await;
        fixture
            .forge
            .update_pull_request(
                &pull_request.id,
                UpdatePullRequest {
                    add_labels: vec![NEEDS_HUMAN_LABEL.to_string()],
                    expected_version: Some(pull_request.version),
                    ..UpdatePullRequest::default()
                },
            )
            .await
            .unwrap();

        assert_eq!(
            fixture.recover().await,
            InterruptedCiRecoveryOutcome::Suppressed
        );
        let suppressed = fixture.fresh().await;
        assert!(requires_human_attention(&suppressed.labels));
        assert!(
            parse_metadata_block(&suppressed.body)
                .unwrap()
                .unwrap()
                .interrupted_ci_recovery
                .is_none()
        );
        assert!(
            fixture
                .forge
                .list_pull_request_comments(&suppressed.id)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(fixture.forge.ci_retry_requests().len(), 1);
    });
}
