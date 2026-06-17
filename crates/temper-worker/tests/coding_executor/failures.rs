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
fn transient_agent_error_maps_to_transient_failure() {
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
    });
}
