use super::support::*;

#[test]
fn success_path_commits_pushes_and_reports_branch() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let agent = AgentBehavior::Success.runner();
        let executor = fixture.executor(agent.clone(), true);

        let outcome = executor
            .execute(assign("agent/pr-for-code-7", "pr-for-code-7"))
            .await;

        let (branch_name, head_sha, summary) = expect_success(outcome);
        assert_eq!(branch_name, "agent/pr-for-code-7");
        assert_is_sha(&head_sha);
        assert_eq!(summary.as_deref(), Some("did the work"));

        assert_eq!(
            git_output([
                "-C",
                path_str(&fixture.origin),
                "rev-parse",
                "refs/heads/agent/pr-for-code-7",
            ]),
            head_sha
        );
        assert_eq!(
            git_output([
                "-C",
                path_str(&fixture.origin),
                "log",
                "-1",
                "--format=%s",
                "refs/heads/agent/pr-for-code-7",
            ]),
            "Implement pr-for-code-7"
        );
        assert_eq!(
            git_output([
                "-C",
                path_str(&fixture.origin),
                "log",
                "-1",
                "--format=%b",
                "refs/heads/agent/pr-for-code-7",
            ]),
            "Closes #7"
        );
        assert_eq!(
            git_output([
                "-C",
                path_str(&fixture.origin),
                "log",
                "-1",
                "--format=%an <%ae>|%cn <%ce>",
                "refs/heads/agent/pr-for-code-7",
            ]),
            "Smith Engineer <smith-engineer@example.test>|Smith Engineer <smith-engineer@example.test>"
        );
        assert_eq!(
            git_output([
                "-C",
                path_str(&fixture.origin),
                "show",
                "refs/heads/agent/pr-for-code-7:agent-output.txt",
            ]),
            "agent diff"
        );
    });
}

#[test]
fn success_path_carries_structured_plan_details() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let executor = fixture.executor(AgentBehavior::SuccessWithPlan.runner(), true);

        let outcome = executor
            .execute(assign("agent/pr-for-code-7", "pr-for-code-7"))
            .await;

        let JobOutcome::Success {
            repos: _,
            summary,
            details,
        } = outcome
        else {
            panic!("expected success outcome");
        };
        assert_eq!(summary.as_deref(), Some("did the planned work"));
        assert_eq!(
            details,
            Some(json!({"plan":{"phases":["Write test","Implement fix"]}}))
        );
    });
}

#[test]
fn workspace_is_reused_across_successful_jobs_for_same_repo_and_role() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let executor = fixture.executor(AgentBehavior::Success.runner(), true);

        expect_success(
            executor
                .execute(assign("agent/pr-for-code-7", "pr-for-code-7"))
                .await,
        );
        let workspace_path = fixture.workspace_root.join("engineer/service");
        assert!(workspace_path.exists());
        let sentinel = workspace_path.join(".git/smith-sentinel");
        fs::write(&sentinel, "keep object cache").expect("write sentinel");

        let (branch_name, head_sha, _) = expect_success(
            executor
                .execute(assign("agent/pr-for-code-8", "pr-for-code-8"))
                .await,
        );

        assert_eq!(branch_name, "agent/pr-for-code-8");
        assert!(
            sentinel.exists(),
            "prepare must reuse the existing checkout"
        );
        assert_eq!(
            git_output([
                "-C",
                path_str(&fixture.origin),
                "rev-parse",
                "refs/heads/agent/pr-for-code-8",
            ]),
            head_sha
        );
    });
}

#[test]
fn zero_diff_maps_to_permanent_failure() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let executor = fixture.executor(AgentBehavior::NoDiff.runner(), true);

        let outcome = executor
            .execute(assign("agent/pr-for-code-7", "pr-for-code-7"))
            .await;

        let message = expect_failure_class(outcome, FailureClass::Permanent);
        assert!(
            message.contains("agent produced no diff"),
            "unexpected message: {message}"
        );
    });
}

#[test]
fn verdict_result_maps_to_permanent_failure() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let executor = fixture.executor(AgentBehavior::Verdict.runner(), true);

        let outcome = executor
            .execute(assign("agent/pr-for-code-7", "pr-for-code-7"))
            .await;

        let message = expect_failure_class(outcome, FailureClass::Permanent);
        assert!(
            message.contains("needs_design"),
            "message should name the unsupported verdict: {message}"
        );
    });
}

#[test]
fn writable_job_with_allowed_escalation_verdict_returns_verdict() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let executor = fixture.executor(AgentBehavior::WritableVerdict.runner(), true);

        let (verdict, body, summary, children) = expect_verdict(
            executor
                .execute(assign_with_context(
                    "pr-for-code-7",
                    writable_job_context_with_allowed_verdicts(
                        "agent/pr-for-code-7",
                        "pr-for-code-7",
                        &["needs_architect"],
                    ),
                ))
                .await,
        );

        assert_eq!(verdict, "needs_architect");
        assert_eq!(body.as_deref(), Some("blocked"));
        assert_eq!(summary.as_deref(), Some("cannot proceed"));
        assert!(children.is_empty());
        assert_no_origin_branch(&fixture, "agent/pr-for-code-7");
        assert_workspace_clean(&fixture, "engineer");
    });
}

#[test]
fn plan_only_empty_commit_is_not_a_successful_product_diff() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let executor = fixture.executor(AgentBehavior::PlanOnlyEmptyCommit.runner(), true);

        let outcome = executor
            .execute(assign("agent/pr-for-code-10", "pr-for-code-10"))
            .await;

        let message = expect_failure_class(outcome, FailureClass::Permanent);
        assert!(
            message.contains("agent produced no diff"),
            "unexpected message: {message}"
        );
        assert_no_origin_branch(&fixture, "agent/pr-for-code-10");
    });
}

#[test]
fn checkpoint_committed_work_with_clean_tree_succeeds() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let executor = fixture.executor(AgentBehavior::CheckpointCommits.runner(), true);

        let outcome = executor
            .execute(assign("agent/pr-for-code-11", "pr-for-code-11"))
            .await;

        let (branch_name, head_sha, summary) = expect_success(outcome);
        assert_eq!(branch_name, "agent/pr-for-code-11");
        assert_eq!(summary.as_deref(), Some("checkpointed the work"));
        assert_eq!(
            git_output([
                "-C",
                path_str(&fixture.origin),
                "rev-parse",
                "refs/heads/agent/pr-for-code-11",
            ]),
            head_sha
        );
        assert_eq!(
            git_output([
                "-C",
                path_str(&fixture.origin),
                "log",
                "-1",
                "--format=%s",
                "refs/heads/agent/pr-for-code-11",
            ]),
            "checkpoint(step 2): push checkpoint"
        );
    });
}

/// Phase 6b: a re-dispatch for the same branch resumes from the pushed remote
/// branch (the prior dispatch's checkpoints) instead of resetting to base.
#[test]
fn redispatch_resumes_from_pushed_work_branch() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let first = AgentBehavior::CheckpointCommits.runner();
        let executor = fixture.executor(first, true);
        let (_, first_head, _) = expect_success(
            executor
                .execute(assign("agent/pr-for-code-12", "pr-for-code-12"))
                .await,
        );

        // Second dispatch, same branch: the runner must observe the prior
        // checkpoint as HEAD, not a fresh base checkout.
        let second = AgentBehavior::Success.runner();
        let executor = fixture.executor(second.clone(), true);
        expect_success(
            executor
                .execute(assign("agent/pr-for-code-12", "pr-for-code-12"))
                .await,
        );
        assert_eq!(
            second.observed_head_sha(),
            first_head,
            "prepare must resume from the pushed work branch"
        );
    });
}
