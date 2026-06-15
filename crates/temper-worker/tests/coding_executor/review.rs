use super::support::*;

#[test]
fn review_job_returns_approve_verdict() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let agent = AgentBehavior::ReviewApprove.runner();
        let executor = fixture.executor(agent.clone(), true);

        let (verdict, body, summary, children) = expect_verdict(
            executor
                .execute(pr_assign("agent/review-7", "review-7", pr_job_context))
                .await,
        );

        assert_eq!(verdict, "approve");
        assert_eq!(body, None);
        assert_eq!(summary.as_deref(), Some("looks good"));
        assert!(children.is_empty());
        // The runner ran in the prepared PR-head checkout: it observed the PR head
        // sha, confirming the executor checked out `refs/pull/7/head`.
        assert_eq!(agent.observed_head_sha(), fixture.pull_request_head_sha);
        assert_no_origin_branch(&fixture, "agent/review-7");
        assert_no_extra_origin_head_branches(&fixture, &["main"]);
    });
}

#[test]
fn review_job_changes_verdict_passes_review_body_through() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let executor = fixture.executor(AgentBehavior::ReviewChanges.runner(), true);

        let (verdict, body, summary, children) = expect_verdict(
            executor
                .execute(pr_assign(
                    "agent/review-changes-7",
                    "review-changes-7",
                    pr_job_context,
                ))
                .await,
        );

        assert_eq!(verdict, "changes");
        assert_eq!(body.as_deref(), Some("please add error handling"));
        assert_eq!(summary.as_deref(), Some("needs error handling"));
        assert!(children.is_empty());
        assert_no_origin_branch(&fixture, "agent/review-changes-7");
        assert_no_extra_origin_head_branches(&fixture, &["main"]);
        assert_workspace_clean(&fixture, "reviewer");
    });
}

#[test]
fn review_job_missing_verdict_is_permanent_failure() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let executor = fixture.executor(AgentBehavior::ReviewMissingVerdict.runner(), true);

        let outcome = executor
            .execute(pr_assign(
                "agent/review-missing-verdict-7",
                "review-missing-verdict-7",
                pr_job_context,
            ))
            .await;

        let message = expect_failure_class(outcome, FailureClass::Permanent);
        assert!(
            message.contains("read-only job returned no verdict"),
            "unexpected message: {message}"
        );
        assert_no_origin_branch(&fixture, "agent/review-missing-verdict-7");
        assert_no_extra_origin_head_branches(&fixture, &["main"]);
    });
}

#[test]
fn review_job_undeclared_verdict_is_permanent_failure() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let executor = fixture.executor(AgentBehavior::ReviewUndeclaredVerdict.runner(), true);

        let outcome = executor
            .execute(pr_assign(
                "agent/review-undeclared-7",
                "review-undeclared-7",
                pr_job_context,
            ))
            .await;

        let message = expect_failure_class(outcome, FailureClass::Permanent);
        assert!(
            message.contains("merge_now"),
            "message should name the emitted verdict: {message}"
        );
        assert!(
            message.contains("approve")
                && message.contains("changes")
                && message.contains("escalate"),
            "message should name the allowed vocabulary: {message}"
        );
        assert_no_origin_branch(&fixture, "agent/review-undeclared-7");
        assert_no_extra_origin_head_branches(&fixture, &["main"]);
    });
}
