use super::support::*;

#[test]
fn read_only_job_returns_verdict_and_body() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let executor = fixture.executor(AgentBehavior::ReadOnlyVerdict.runner(), true);

        let (verdict, body, summary, children) = expect_verdict(
            executor
                .execute(assign_with_context(
                    "triage-7",
                    read_only_job_context("agent/triage-7", "triage-7"),
                ))
                .await,
        );

        assert_eq!(verdict, "ready_code");
        assert_eq!(body.as_deref(), Some("rewritten"));
        assert_eq!(summary.as_deref(), Some("did triage"));
        assert!(children.is_empty());
        assert_no_origin_branch(&fixture, "agent/triage-7");
    });
}

#[test]
fn read_only_job_materializes_missing_target_base_without_head_push() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let executor = fixture.executor(AgentBehavior::ReadOnlyVerdict.runner(), true);
        let context =
            read_only_job_context("agent/plan-7", "plan-7").with_base_branch("feature/plan-7");

        let (verdict, body, summary, children) = expect_verdict(
            executor
                .execute(assign_with_context("plan-7", context))
                .await,
        );

        assert_eq!(verdict, "ready_code");
        assert_eq!(body.as_deref(), Some("rewritten"));
        assert_eq!(summary.as_deref(), Some("did triage"));
        assert!(children.is_empty());
        assert_origin_branch_exists(&fixture, "feature/plan-7");
        assert_no_origin_branch(&fixture, "agent/plan-7");
    });
}

#[test]
fn read_only_job_with_diff_still_returns_verdict_without_push() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let executor = fixture.executor(AgentBehavior::ReadOnlyVerdictWithDiff.runner(), true);

        let (verdict, body, summary, children) = expect_verdict(
            executor
                .execute(assign_with_context(
                    "triage-with-diff-7",
                    read_only_job_context("agent/triage-with-diff-7", "triage-with-diff-7"),
                ))
                .await,
        );

        assert_eq!(verdict, "ready_code");
        assert_eq!(body.as_deref(), Some("rewritten"));
        assert_eq!(summary.as_deref(), Some("did triage"));
        assert!(children.is_empty());
        assert_no_origin_branch(&fixture, "agent/triage-with-diff-7");
        assert_workspace_clean(&fixture, "architect", "triage-with-diff-7");
    });
}

#[test]
fn read_only_breakdown_verdict_passes_children_through() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let executor = fixture.executor(AgentBehavior::ReadOnlyBreakdownVerdict.runner(), true);
        let mut context = read_only_job_context("agent/breakdown-7", "breakdown-7");
        context.allowed_verdicts = vec!["needs_breakdown".to_string()];

        let (verdict, body, summary, children) = expect_verdict(
            executor
                .execute(assign_with_context("breakdown-7", context))
                .await,
        );

        assert_eq!(verdict, "needs_breakdown");
        assert_eq!(body, None);
        assert_eq!(summary.as_deref(), Some("planned breakdown"));
        assert_eq!(
            children,
            vec![
                JobChild {
                    slug: "api-schema".to_string(),
                    title: "Define the API schema".to_string(),
                    body: "Write the shared API schema.".to_string(),
                    kind: None,
                    labels: vec!["code".to_string(), "ready".to_string()],
                    depends_on: Vec::new(),
                    target_repo: None,
                },
                JobChild {
                    slug: "web-client".to_string(),
                    title: "Implement the web client".to_string(),
                    body: "Build the web client against the API schema.".to_string(),
                    kind: None,
                    labels: Vec::new(),
                    depends_on: vec!["api-schema".to_string()],
                    target_repo: Some("acme/other".to_string()),
                },
            ]
        );
        assert_no_origin_branch(&fixture, "agent/breakdown-7");
    });
}

#[test]
fn worker_rejects_agent_result_that_violates_verdict_contract() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let executor = fixture.executor(AgentBehavior::ReadOnlyVerdict.runner(), true);
        let mut context = read_only_job_context("agent/contract-7", "contract-7");
        context.verdict_contracts.insert(
            "ready_code".to_string(),
            temper_verdict::VerdictContract {
                min_children: 1,
                allowed_child_kinds: vec!["code".to_string()],
                ..Default::default()
            },
        );

        let outcome = executor
            .execute(assign_with_context("contract-7", context))
            .await;

        let message = expect_failure_class(outcome, FailureClass::Protocol);
        assert!(message.contains("violates its workflow verdict contract"));
        assert!(message.contains("requires at least 1 child product(s), received 0"));
        assert_no_origin_branch(&fixture, "agent/contract-7");
    });
}

#[test]
fn read_only_job_without_verdict_is_permanent() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let executor = fixture.executor(AgentBehavior::NoDiff.runner(), true);

        let outcome = executor
            .execute(assign_with_context(
                "triage-no-verdict-7",
                read_only_job_context("agent/triage-no-verdict-7", "triage-no-verdict-7"),
            ))
            .await;

        let message = expect_failure_class(outcome, FailureClass::Permanent);
        assert!(
            message.contains("read-only job returned no verdict"),
            "unexpected message: {message}"
        );
        assert_no_origin_branch(&fixture, "agent/triage-no-verdict-7");
    });
}

#[test]
fn read_only_job_with_undeclared_verdict_is_permanent() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let executor = fixture.executor(AgentBehavior::UndeclaredVerdict.runner(), true);

        let outcome = executor
            .execute(assign_with_context(
                "triage-undeclared-7",
                read_only_job_context("agent/triage-undeclared-7", "triage-undeclared-7"),
            ))
            .await;

        let message = expect_failure_class(outcome, FailureClass::Permanent);
        assert!(
            message.contains("needs_breakdown"),
            "message should name the emitted verdict: {message}"
        );
        assert!(
            message.contains("ready_code") && message.contains("needs_design"),
            "message should name the allowed vocabulary: {message}"
        );
        assert_no_origin_branch(&fixture, "agent/triage-undeclared-7");
    });
}
