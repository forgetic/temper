use super::support::*;

#[test]
fn coding_executor_binds_fake_progress_to_one_attempt() {
    use std::sync::Mutex;

    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let observed = Arc::new(Mutex::new(Vec::new()));
        let observed_for_factory = Arc::clone(&observed);
        let executor = fixture
            .executor(AgentBehavior::Success.runner(), true)
            .with_progress_reporter_factory(move |_job_id, attempt_id| {
                let observed = Arc::clone(&observed_for_factory);
                JobProgressReporter::new(attempt_id.to_string(), move |progress| {
                    observed.lock().unwrap().push(progress);
                })
            });

        expect_success(
            executor
                .execute(assign("agent/pr-for-code-7", "attempt-progress-7"))
                .await,
        );

        let observed = observed.lock().unwrap();
        assert_eq!(observed.len(), 2);
        assert!(!observed[0].attempt_id.is_empty());
        assert_eq!(observed[0].attempt_id, observed[1].attempt_id);
        assert!(matches!(
            observed[0].frame.event,
            AgentLifecycleEventV1::ModelStarted { .. }
        ));
        assert!(matches!(
            observed[1].frame.event,
            AgentLifecycleEventV1::AgentFinished { .. }
        ));
    });
}

#[test]
fn artifact_context_bundle_is_copied_without_reconstruction() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let agent = AgentBehavior::Success.runner();
        let executor = fixture.executor(agent.clone(), true);
        let bundle = json!({
            "version":1,
            "repository":{"id":"repo-1","path":"acme/service"},
            "artifact_type":"issue",
            "primary":{"artifact":{"repository":{"id":"repo-1","path":"acme/service"},"artifact_type":"issue","number":7},"title":"Primary","body":"Detailed issue body","labels":["code"],"state":"open","workflow_kind":"code"},
            "diagnostics":[{"code":"content_truncated","message":"kept verbatim"}],
            "truncation":{"depth_exceeded":false,"count_exceeded":false,"content_truncated":true}
        });
        let mut assignment = assign("agent/pr-for-code-7", "artifact-context-7");
        assignment.job_payload["artifact_context"] = bundle.clone();

        expect_success(executor.execute(assignment).await);

        let context = agent.captured_context();
        assert_eq!(
            serde_json::to_value(context.artifact_context.expect("bundle copied")).unwrap(),
            bundle
        );
        let legacy: Value = serde_json::from_str(&context.work_item.context).unwrap();
        assert_eq!(legacy["artifact"]["body"], "Detailed issue body");
    });
}

#[test]
fn w3c_trace_context_propagates_from_assignment_to_agent_workspace() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let agent = AgentBehavior::Success.runner();
        let executor = fixture.executor(agent.clone(), true);
        let mut assignment = assign("agent/pr-for-code-7", "trace-context-7");
        let trace_context = temper_protocol_worker::W3cTraceContext {
            traceparent: "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".into(),
            tracestate: Some("vendor=opaque".into()),
        };
        assignment.trace_context = Some(trace_context.clone());

        expect_success(executor.execute(assignment).await);

        assert_eq!(agent.captured_context().trace_context, Some(trace_context));
    });
}

#[test]
fn context_shape_matches_temper_coding_agent_contract() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let agent = AgentBehavior::Success.runner();
        let executor = fixture.executor(agent.clone(), true);

        expect_success(
            executor
                .execute(assign("agent/pr-for-code-7", "pr-for-code-7"))
                .await,
        );

        let context = agent.captured_context();
        assert_workspace_context(
            &context,
            ExpectedWorkspaceContext {
                role: "engineer",
                queue: "code_ready",
                action: "open_pr",
                kind: "code",
                checkout: "writable",
                allowed_verdicts: &[],
                branch_hint: "agent/pr-for-code-7",
                correlation_key: "pr-for-code-7",
                target: "Issue { number: ItemNumber(7) }",
                artifact_type: "issue",
            },
        );
    });
}

#[test]
fn context_shape_passes_through_read_only_capability_and_verdicts() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let agent = AgentBehavior::ReadOnlyVerdict.runner();
        let executor = fixture.executor(agent.clone(), true);

        expect_verdict(
            executor
                .execute(assign_with_context(
                    "triage-7",
                    read_only_job_context("agent/triage-7", "triage-7"),
                ))
                .await,
        );

        let context = agent.captured_context();
        assert_workspace_context(
            &context,
            ExpectedWorkspaceContext {
                role: "architect",
                queue: "design_review",
                action: "triage_intake",
                kind: "triage",
                checkout: "read_only",
                allowed_verdicts: &["ready_code", "needs_design"],
                branch_hint: "agent/triage-7",
                correlation_key: "triage-7",
                target: "Issue { number: ItemNumber(7) }",
                artifact_type: "issue",
            },
        );
    });
}

#[test]
fn review_context_shape_carries_pull_request_target() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let agent = AgentBehavior::ReviewApprove.runner();
        let executor = fixture.executor(agent.clone(), true);

        expect_verdict(
            executor
                .execute(pr_assign("agent/review-7", "review-7", pr_job_context))
                .await,
        );

        let context = agent.captured_context();
        assert_workspace_context(
            &context,
            ExpectedWorkspaceContext {
                role: "reviewer",
                queue: "pr_needs_review",
                action: "review_pr",
                kind: "implementation_pr",
                checkout: "pull_request_read_only",
                allowed_verdicts: &["approve", "changes", "escalate"],
                branch_hint: "agent/review-7",
                correlation_key: "review-7",
                target: "PullRequest { number: ItemNumber(7) }",
                artifact_type: "pull_request",
            },
        );
    });
}

struct ExpectedWorkspaceContext<'a> {
    role: &'a str,
    queue: &'a str,
    action: &'a str,
    kind: &'a str,
    checkout: &'a str,
    allowed_verdicts: &'a [&'a str],
    branch_hint: &'a str,
    correlation_key: &'a str,
    target: &'a str,
    artifact_type: &'a str,
}

fn assert_workspace_context(context: &WorkspaceContext, expected: ExpectedWorkspaceContext<'_>) {
    let primary = context.primary().expect("primary repo present");
    assert_eq!(primary.id, "acme/service");
    assert_eq!(primary.owner, "acme");
    assert_eq!(primary.name, "service");
    assert_eq!(primary.default_branch, "main");
    assert_eq!(primary.dir, "service");
    assert!(primary.is_writable());
    assert_eq!(context.work_item.role, expected.role);
    assert_eq!(context.work_item.queue, expected.queue);
    assert_eq!(context.action, expected.action);
    assert_eq!(context.work_item.kind, expected.kind);
    assert_eq!(context.work_item.target, expected.target);
    assert_eq!(primary.base_branch, "main");
    assert_eq!(primary.branch_hint.as_deref(), Some(expected.branch_hint));
    assert_eq!(context.correlation_key, expected.correlation_key);
    assert_eq!(context.checkout.as_deref(), Some(expected.checkout));
    assert_eq!(
        context.allowed_verdicts,
        expected
            .allowed_verdicts
            .iter()
            .map(|verdict| (*verdict).to_string())
            .collect::<Vec<_>>()
    );
    assert_eq!(context.guidance.role_guidance, None);
    assert_eq!(context.guidance.tool_guidance, None);
    assert!(context.guidance.tool_constraints.is_empty());

    let inner: Value =
        serde_json::from_str(&context.work_item.context).expect("inner work item JSON parses");
    assert_eq!(inner["repository"], "acme/service");
    assert_eq!(inner["role"], expected.role);
    assert_eq!(inner["queue"], expected.queue);
    assert_eq!(inner["action"], expected.action);
    assert_eq!(inner["kind"], expected.kind);
    assert_eq!(inner["artifact"]["type"], expected.artifact_type);
    assert_eq!(inner["artifact"]["number"], 7);
    assert_eq!(inner["artifact"]["title"], "Implement the thing");
    assert_eq!(inner["artifact"]["body"], "Detailed issue body");
    assert_eq!(inner["artifact"]["labels"], json!(["code", "ready"]));
    assert_eq!(inner["artifact"]["state"], "Open");
}
