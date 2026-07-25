use super::support::*;

#[test]
fn malformed_payload_maps_to_protocol_failure() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let executor = fixture.executor(AgentBehavior::NoDiff.runner(), true);

        let outcome = executor
            .execute(Assign {
                job_payload: json!({"nope": true}),
                ..assign("agent/pr-for-code-7", "pr-for-code-7")
            })
            .await;

        expect_failure_class(outcome, FailureClass::Protocol);
    });
}

#[test]
fn missing_enriched_artifact_maps_to_protocol_failure() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let executor = fixture.executor(AgentBehavior::NoDiff.runner(), true);
        let mut context = job_context("agent/pr-for-code-7", "pr-for-code-7");
        context.artifact = None;

        let outcome = executor
            .execute(Assign {
                job_payload: context.to_payload(),
                ..assign("agent/pr-for-code-7", "pr-for-code-7")
            })
            .await;

        let message = expect_failure_class(outcome, FailureClass::Protocol);
        assert!(
            message.contains("artifact"),
            "message should name missing field: {message}"
        );
    });
}

#[test]
fn missing_assigned_action_maps_to_protocol_failure() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let executor = fixture.executor(AgentBehavior::NoDiff.runner(), true);
        let mut context = job_context("agent/pr-for-code-7", "pr-for-code-7");
        context.action = None;

        let outcome = executor
            .execute(Assign {
                job_payload: context.to_payload(),
                ..assign("agent/pr-for-code-7", "pr-for-code-7")
            })
            .await;

        let message = expect_failure_class(outcome, FailureClass::Protocol);
        assert!(
            message.contains("action"),
            "message should name missing action: {message}"
        );
    });
}

#[test]
fn missing_role_identity_maps_to_permanent_failure() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let executor = fixture.executor(AgentBehavior::NoDiff.runner(), false);

        let outcome = executor
            .execute(assign("agent/pr-for-code-7", "pr-for-code-7"))
            .await;

        let message = expect_failure_class(outcome, FailureClass::Permanent);
        assert!(
            message.contains("worker has no git identity for role engineer"),
            "unexpected message: {message}"
        );
    });
}

#[test]
fn transient_agent_error_maps_to_transient_failure_without_consuming_model_budget() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let executor = fixture.executor(AgentBehavior::TransientError.runner(), true);

        let outcome = executor
            .execute(assign("agent/pr-for-code-7", "pr-for-code-7"))
            .await;

        let message = expect_failure_class(outcome, FailureClass::Transient);
        assert!(
            message.contains("provider transport reset"),
            "transient error message missing: {message}"
        );
        let ledger = temper_worker::AgentSessionStore::for_workspace_root(
            &fixture.workspace_root,
            "engineer",
            "pr-for-code-7",
        )
        .unwrap()
        .load_ledger_sync()
        .unwrap()
        .unwrap();
        assert_eq!(ledger.consecutive_terminal_count, 0);
        assert_eq!(ledger.accounted_attempt_id, None);
    });
}

#[test]
fn writable_engineer_verdict_resets_recovery_before_the_workstream_is_requeued() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let branch = "agent/pr-for-code-7";
        let coordination_key = "verdict-model-recovery-reset";
        let store = temper_worker::AgentSessionStore::for_workspace_root(
            &fixture.workspace_root,
            "engineer",
            coordination_key,
        )
        .unwrap();

        let failing_executor = fixture.executor(AgentBehavior::RetryableModelError.runner(), true);
        for count in 1..=2 {
            let mut assignment = assign(branch, coordination_key);
            assignment.attempt_id = Some(format!("attempt-before-verdict-{count}"));
            let recovery = match failing_executor.execute(assignment).await {
                JobOutcome::Failure {
                    class,
                    model_failure: Some(model_failure),
                    session_recovery: Some(recovery),
                    ..
                } => {
                    assert_eq!(class, FailureClass::Transient);
                    assert!(model_failure.retryable);
                    recovery
                }
                other => panic!("expected retryable model failure, got {other:?}"),
            };
            assert_eq!(
                recovery.action,
                temper_protocol_worker::SessionRecoveryActionV1::RetryCurrentSession
            );
            assert_eq!(recovery.failure_epoch, 1);
            assert_eq!(recovery.failure_count, count);
        }

        let before_verdict = store.load_ledger_sync().unwrap().unwrap();
        assert_eq!(before_verdict.failure_epoch, 1);
        assert_eq!(before_verdict.consecutive_terminal_count, 2);
        assert!(!before_verdict.rotation_consumed);
        let active_session = before_verdict.active_session.session_id.clone();

        let verdict_executor = fixture.executor(AgentBehavior::WritableVerdict.runner(), true);
        let mut verdict_assignment = assign_with_context(
            coordination_key,
            writable_job_context_with_allowed_verdicts(
                branch,
                coordination_key,
                &["needs_architect"],
            ),
        );
        verdict_assignment.attempt_id = Some("attempt-authoritative-verdict".to_string());
        let (verdict, _, _, _) = expect_verdict(verdict_executor.execute(verdict_assignment).await);
        assert_eq!(verdict, "needs_architect");

        let after_verdict = store.load_ledger_sync().unwrap().unwrap();
        assert_eq!(after_verdict.failure_epoch, 2);
        assert_eq!(after_verdict.consecutive_terminal_count, 0);
        assert!(!after_verdict.rotation_consumed);
        assert_eq!(after_verdict.accounted_attempt_id, None);
        assert_eq!(after_verdict.recovery_decision, None);
        assert_eq!(after_verdict.active_session.session_id, active_session);

        // Requeue the same coordination-scoped workstream. The fresh epoch gets
        // all three retryable runs before consuming its one rotation.
        let requeued_executor = fixture.executor(AgentBehavior::RetryableModelError.runner(), true);
        for count in 1..=3 {
            let mut assignment = assign(branch, coordination_key);
            assignment.attempt_id = Some(format!("attempt-after-verdict-{count}"));
            let recovery = match requeued_executor.execute(assignment).await {
                JobOutcome::Failure {
                    class,
                    model_failure: Some(model_failure),
                    session_recovery: Some(recovery),
                    ..
                } => {
                    assert_eq!(class, FailureClass::Transient);
                    assert!(model_failure.retryable);
                    recovery
                }
                other => panic!("expected bounded model failure, got {other:?}"),
            };
            let expected_action = if count < 3 {
                temper_protocol_worker::SessionRecoveryActionV1::RetryCurrentSession
            } else {
                temper_protocol_worker::SessionRecoveryActionV1::RotateSession
            };
            assert_eq!(recovery.action, expected_action);
            assert_eq!(recovery.failure_epoch, 2);
            assert_eq!(recovery.failure_count, count);
            assert_eq!(recovery.current_session_id, active_session);
        }

        let after_full_budget = store.load_ledger_sync().unwrap().unwrap();
        assert_eq!(after_full_budget.failure_epoch, 2);
        assert_eq!(after_full_budget.consecutive_terminal_count, 0);
        assert!(after_full_budget.rotation_consumed);
        assert_eq!(
            after_full_budget
                .prior_session
                .as_ref()
                .unwrap()
                .consecutive_terminal_count,
            3
        );
        assert_ne!(after_full_budget.active_session.session_id, active_session);
    });
}

#[test]
fn non_retryable_model_failures_rotate_once_then_park_without_losing_workspace() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let branch = "agent/pr-for-code-7";
        let coordination_key = "bounded-model-recovery";
        let runner = AgentBehavior::NonRetryableModelError.runner();
        let executor = fixture.executor(runner.clone(), true);
        let initial_head = git_output([
            "-C",
            path_str(&fixture.origin),
            "rev-parse",
            "refs/heads/main",
        ]);
        let mut first_assign = assign(branch, coordination_key);
        first_assign.attempt_id = Some("attempt-model-first".to_string());

        let first_outcome = executor.execute(first_assign).await;
        let (first_message, first_recovery) = match first_outcome {
            JobOutcome::Failure {
                class,
                model_failure: Some(model_failure),
                session_recovery: Some(recovery),
                message,
            } => {
                assert_eq!(class, FailureClass::Transient);
                assert!(!model_failure.retryable);
                (message, recovery)
            }
            other => panic!("expected typed rotation failure, got {other:?}"),
        };
        assert_eq!(
            first_recovery.action,
            temper_protocol_worker::SessionRecoveryActionV1::RotateSession
        );
        let new_session = first_recovery
            .new_session_id
            .clone()
            .expect("rotation creates one new session");
        assert_ne!(first_recovery.current_session_id, new_session);
        assert!(first_message.contains(&first_recovery.current_session_id));
        assert!(first_message.contains(&new_session));
        assert_eq!(
            runner
                .captured_context()
                .agent_session
                .expect("first session")
                .session_id,
            first_recovery.current_session_id
        );

        let checkout = fixture
            .workspace_root
            .join("engineer")
            .join(coordination_key)
            .join("service");
        let assert_dirty_work_preserved = || {
            assert_eq!(
                fs::read_to_string(checkout.join("README.md")).unwrap(),
                "# seed\nvaluable tracked work\n"
            );
            assert_eq!(
                fs::read_to_string(checkout.join("model-untracked.txt")).unwrap(),
                "valuable untracked work\n"
            );
            let status = git_output([
                "-C",
                path_str(&checkout),
                "status",
                "--porcelain=v1",
                "--untracked-files=all",
            ]);
            assert!(
                status.contains(" M README.md"),
                "tracked work missing: {status}"
            );
            assert!(
                status.contains("?? model-untracked.txt"),
                "untracked work missing: {status}"
            );
        };
        assert_dirty_work_preserved();
        assert_eq!(
            git_output(["-C", path_str(&checkout), "stash", "list"]),
            "",
            "the ledger rotation itself must not stash workspace changes"
        );
        assert_eq!(
            git_output(["-C", path_str(&checkout), "rev-parse", "HEAD"]),
            initial_head,
            "the ledger rotation itself must not commit or reset workspace changes"
        );
        assert!(
            !checkout
                .with_file_name("service.temper-quarantine")
                .exists(),
            "the ledger rotation itself must not quarantine the checkout"
        );

        let store = temper_worker::AgentSessionStore::for_workspace_root(
            &fixture.workspace_root,
            "engineer",
            coordination_key,
        )
        .unwrap();
        let after_rotation = store.load_ledger_sync().unwrap().unwrap();
        assert_eq!(after_rotation.active_session.session_id, new_session);
        assert_eq!(
            after_rotation
                .prior_session
                .as_ref()
                .unwrap()
                .session
                .session_id,
            first_recovery.current_session_id
        );

        let mut second_assign = assign(branch, coordination_key);
        second_assign.attempt_id = Some("attempt-model-second".to_string());
        let second_outcome = executor.execute(second_assign.clone()).await;
        let replay_expected = second_outcome.clone();
        let second_recovery = match second_outcome {
            JobOutcome::Failure {
                class,
                model_failure: Some(model_failure),
                session_recovery: Some(recovery),
                message,
            } => {
                assert_eq!(class, FailureClass::Permanent);
                assert!(!model_failure.retryable);
                assert!(message.contains(&new_session));
                recovery
            }
            other => panic!("expected typed park failure, got {other:?}"),
        };
        assert_eq!(
            second_recovery.action,
            temper_protocol_worker::SessionRecoveryActionV1::ParkForHuman
        );
        assert_eq!(second_recovery.current_session_id, new_session);
        assert_eq!(
            second_recovery.prior_session_id.as_deref(),
            Some(first_recovery.current_session_id.as_str())
        );
        assert_eq!(second_recovery.new_session_id, None);
        assert_dirty_work_preserved();

        // The exact same daemon attempt is answered from the durable boundary;
        // the panic runner proves no third model session is invoked.
        let replay = fixture
            .executor(AgentBehavior::UnexpectedRun.runner(), true)
            .execute(second_assign)
            .await;
        assert_eq!(replay, replay_expected);
        let after_replay = store.load_ledger_sync().unwrap().unwrap();
        assert_eq!(after_replay.active_session.session_id, new_session);
        assert_eq!(
            after_replay.prior_session.unwrap().session.session_id,
            first_recovery.current_session_id
        );
        assert_dirty_work_preserved();
    });
}
