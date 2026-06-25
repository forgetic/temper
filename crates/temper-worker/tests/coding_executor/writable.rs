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
fn scoped_workspace_is_reused_for_same_coordination_key() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let executor = fixture.executor(AgentBehavior::Success.runner(), true);

        let (_, first_head, _) = expect_success(
            executor
                .execute(assign("agent/pr-for-code-7", "pr-for-code-7"))
                .await,
        );
        let workspace_path = fixture
            .workspace_root
            .join("engineer")
            .join("pr-for-code-7")
            .join("service");
        assert!(workspace_path.exists());
        let sentinel = workspace_path.join(".git/smith-sentinel");
        fs::write(&sentinel, "keep object cache").expect("write sentinel");

        let (branch_name, head_sha, _) = expect_success(
            executor
                .execute(assign("agent/pr-for-code-7", "pr-for-code-7"))
                .await,
        );

        assert_eq!(branch_name, "agent/pr-for-code-7");
        assert_eq!(head_sha, first_head);
        assert!(
            sentinel.exists(),
            "prepare must reuse the existing scoped checkout for the same coordination key"
        );
    });
}

#[test]
fn distinct_coordination_keys_use_distinct_checkout_directories() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let executor = fixture.executor(AgentBehavior::Success.runner(), true);

        expect_success(
            executor
                .execute(assign("agent/pr-for-code-7", "pr-for-code-7"))
                .await,
        );
        let first_workspace_path = fixture
            .workspace_root
            .join("engineer")
            .join("pr-for-code-7")
            .join("service");
        assert!(first_workspace_path.exists());
        let first_sentinel = first_workspace_path.join(".git/smith-sentinel");
        fs::write(&first_sentinel, "keep first job object cache").expect("write sentinel");

        let (branch_name, head_sha, _) = expect_success(
            executor
                .execute(assign("agent/pr-for-code-8", "pr-for-code-8"))
                .await,
        );
        let second_workspace_path = fixture
            .workspace_root
            .join("engineer")
            .join("pr-for-code-8")
            .join("service");

        assert_ne!(first_workspace_path, second_workspace_path);
        assert_eq!(branch_name, "agent/pr-for-code-8");
        assert!(second_workspace_path.exists());
        assert!(
            first_sentinel.exists(),
            "preparing the second job must not wipe the first job's checkout"
        );
        assert!(
            !second_workspace_path.join(".git/smith-sentinel").exists(),
            "a distinct coordination key must not reuse the first job's checkout"
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
fn coordination_key_scope_is_encoded_as_one_safe_path_component() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let executor = fixture.executor(AgentBehavior::Success.runner(), true);

        expect_success(
            executor
                .execute(assign("agent/escaped-scope", "../../escape/nested"))
                .await,
        );

        let role_root = fixture.workspace_root.join("engineer");
        let encoded_scope = "%2E%2E%2F%2E%2E%2Fescape%2Fnested";
        assert!(role_root.join(encoded_scope).join("service").exists());
        assert!(
            !fixture
                .workspace_root
                .parent()
                .expect("workspace root has temp parent")
                .join("escape")
                .exists(),
            "an unsanitized coordination key would escape the workspace root"
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
        assert_workspace_clean(&fixture, "engineer", "pr-for-code-7");
    });
}

#[test]
fn empty_checkpoint_commit_is_not_a_successful_product_diff() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let executor = fixture.executor(AgentBehavior::EmptyCheckpointCommit.runner(), true);

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
