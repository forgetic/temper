use super::support::*;

struct StaleGuard;

impl temper_worker::PrFreshnessGuard for StaleGuard {
    fn check<'a>(
        &'a self,
        _check: &'a temper_protocol_agent::PullRequestFreshness,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<(), temper_worker::PrFreshnessFailure>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async {
            Err(temper_worker::PrFreshnessFailure::Stale(
                "pull request merged".to_string(),
            ))
        })
    }
}

struct RecordingFreshGuard {
    checks: std::sync::Mutex<Vec<temper_protocol_agent::PullRequestFreshness>>,
}

impl RecordingFreshGuard {
    fn new() -> Self {
        Self {
            checks: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn checks(&self) -> Vec<temper_protocol_agent::PullRequestFreshness> {
        self.checks.lock().expect("checks lock").clone()
    }
}

impl temper_worker::PrFreshnessGuard for RecordingFreshGuard {
    fn check<'a>(
        &'a self,
        check: &'a temper_protocol_agent::PullRequestFreshness,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<(), temper_worker::PrFreshnessFailure>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            self.checks.lock().expect("checks lock").push(check.clone());
            Ok(())
        })
    }
}

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
fn stale_pr_fix_cancels_before_final_push() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let executor = fixture
            .executor(AgentBehavior::Success.runner(), true)
            .with_pr_freshness_guard(Arc::new(StaleGuard));

        let outcome = executor
            .execute(assign_with_context(
                "pr-for-code-7",
                pr_fix_job_context("agent/pr-for-code-7", "pr-for-code-7"),
            ))
            .await;

        let message = expect_failure_class(outcome, FailureClass::Canceled);
        assert!(message.contains("stale pull request job canceled before push"));
        assert!(message.contains("pull request merged"));
        assert_no_origin_branch(&fixture, "agent/pr-for-code-7");
    });
}

#[test]
fn pr_fix_final_freshness_uses_latest_checkpoint_head() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let guard = Arc::new(RecordingFreshGuard::new());
        let executor = fixture
            .executor(AgentBehavior::CheckpointCommits.runner(), true)
            .with_pr_freshness_guard(guard.clone());

        let outcome = executor
            .execute(assign_with_context(
                "pr-for-code-7",
                pr_fix_job_context("agent/pr-for-code-7", "pr-for-code-7"),
            ))
            .await;

        let (_branch_name, head_sha, summary) = expect_success(outcome);
        assert_eq!(summary.as_deref(), Some("checkpointed the work"));
        let checks = guard.checks();
        assert_eq!(
            checks.len(),
            1,
            "final push freshness should be checked once"
        );
        assert_eq!(checks[0].head_sha.as_deref(), Some(head_sha.as_str()));
        assert_ne!(checks[0].head_sha.as_deref(), Some("assigned-head"));
        assert_eq!(checks[0].queue_condition, None);
        assert!(checks[0].queue_labels.is_empty());
    });
}

#[test]
fn pr_writable_prepares_existing_pr_head_and_pushes_fix_to_same_branch() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let branch = "agent/pr-for-code-7";
        let assigned_head = fixture.seed_pr_head_branch(branch);
        let agent = AgentBehavior::Success.runner();
        let executor = fixture.executor(agent.clone(), true);

        let outcome = executor
            .execute(pr_fix_assign(branch, "pr-for-code-7"))
            .await;

        let (branch_name, head_sha, summary) = expect_success(outcome);
        assert_eq!(branch_name, branch);
        assert_eq!(summary.as_deref(), Some("did the work"));
        assert_eq!(
            agent.observed_head_sha(),
            assigned_head,
            "pull_request_writable must start from the existing PR head"
        );
        assert_ne!(head_sha, assigned_head);
        assert_eq!(
            git_output([
                "-C",
                path_str(&fixture.origin),
                "rev-parse",
                &format!("refs/heads/{branch}"),
            ]),
            head_sha
        );
        assert_eq!(
            git_output([
                "-C",
                path_str(&fixture.origin),
                "rev-parse",
                &format!("refs/heads/{branch}^"),
            ]),
            assigned_head
        );
        assert_eq!(
            git_output([
                "-C",
                path_str(&fixture.origin),
                "log",
                "-1",
                "--format=%s",
                &format!("refs/heads/{branch}"),
            ]),
            "Fix CI for pr-for-code-7"
        );
        assert_eq!(
            git_output([
                "-C",
                path_str(&fixture.origin),
                "show",
                &format!("refs/heads/{branch}:pr-change.txt"),
            ]),
            "pull request change"
        );
        assert_eq!(
            git_output([
                "-C",
                path_str(&fixture.origin),
                "show",
                &format!("refs/heads/{branch}:agent-output.txt"),
            ]),
            "agent diff"
        );
    });
}

#[test]
fn pr_merge_conflict_repair_merges_main_and_pushes_existing_pr_branch() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let branch = "agent/pr-for-code-7";
        let (assigned_head, main_head) = fixture.seed_conflicting_pr_head_branch(branch);
        let agent = AgentBehavior::ResolveMainConflict.runner();
        let executor = fixture.executor(agent.clone(), true);

        let outcome = executor
            .execute(pr_merge_conflict_assign(branch, "pr-for-code-7"))
            .await;

        let (branch_name, head_sha, summary) = expect_success(outcome);
        assert_eq!(branch_name, branch);
        assert_eq!(
            summary.as_deref(),
            Some("resolved merge conflict with main")
        );
        assert_eq!(
            agent.observed_head_sha(),
            assigned_head,
            "conflict repair must start from the existing PR head"
        );
        assert_ne!(head_sha, assigned_head);
        assert_eq!(
            git_output([
                "-C",
                path_str(&fixture.origin),
                "rev-parse",
                &format!("refs/heads/{branch}"),
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
                &format!("refs/heads/{branch}"),
            ]),
            "Resolve merge conflict for pr-for-code-7"
        );
        let parents = git_output([
            "-C",
            path_str(&fixture.origin),
            "log",
            "-1",
            "--format=%P",
            &format!("refs/heads/{branch}"),
        ]);
        assert!(
            parents
                .split_whitespace()
                .any(|parent| parent == assigned_head.as_str()),
            "repair commit parents should include the original PR head {assigned_head}: {parents}"
        );
        assert!(
            parents
                .split_whitespace()
                .any(|parent| parent == main_head.as_str()),
            "repair commit parents should include advanced main {main_head}: {parents}"
        );
        git([
            "-C",
            path_str(&fixture.origin),
            "merge-base",
            "--is-ancestor",
            &main_head,
            &format!("refs/heads/{branch}"),
        ]);
        assert_eq!(
            git_output([
                "-C",
                path_str(&fixture.origin),
                "show",
                &format!("refs/heads/{branch}:conflict.txt"),
            ]),
            "resolved by combining main and pull request changes"
        );
    });
}

#[test]
fn pr_feedback_resumes_saved_engineer_session_for_same_coordination_key() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let branch = "agent/pr-for-code-7";
        let agent = AgentBehavior::Success.runner();
        let executor = fixture.executor(agent.clone(), true);

        expect_success(executor.execute(assign(branch, "pr-for-code-7")).await);
        let initial_session = agent
            .captured_context()
            .agent_session
            .expect("issue job received an agent session");

        // Make the PR-head repair job produce a fresh diff while keeping the
        // same coordination key and saved session.
        fixture.seed_pr_head_branch(branch);
        expect_success(
            executor
                .execute(pr_fix_assign(branch, "pr-for-code-7"))
                .await,
        );
        let feedback_session = agent
            .captured_context()
            .agent_session
            .expect("feedback job received an agent session");

        assert_eq!(feedback_session, initial_session);
    });
}

#[test]
fn corrupt_session_state_falls_back_to_new_session_for_pr_feedback() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let branch = "agent/pr-for-code-7";
        fixture.seed_pr_head_branch(branch);
        let store = temper_worker::AgentSessionStore::for_workspace_root(
            &fixture.workspace_root,
            "engineer",
            "pr-for-code-7",
        )
        .expect("session store");
        fs::create_dir_all(store.path().parent().expect("session parent"))
            .expect("create corrupt session parent");
        fs::write(store.path(), "{not valid json").expect("write corrupt session");

        let agent = AgentBehavior::Success.runner();
        let executor = fixture.executor(agent.clone(), true);
        expect_success(
            executor
                .execute(pr_fix_assign(branch, "pr-for-code-7"))
                .await,
        );

        let fallback_session = agent
            .captured_context()
            .agent_session
            .expect("feedback job received fallback session");
        assert!(!fallback_session.session_id.trim().is_empty());
        assert_eq!(
            store.load_sync().expect("saved fallback session"),
            Some(fallback_session),
            "successful feedback run should replace the corrupt saved state"
        );
    });
}

#[test]
fn pr_writable_no_diff_on_existing_pr_head_is_not_success() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let branch = "agent/pr-for-code-7";
        let assigned_head = fixture.seed_pr_head_branch(branch);
        let executor = fixture.executor(AgentBehavior::NoDiff.runner(), true);

        let outcome = executor
            .execute(pr_fix_assign(branch, "pr-for-code-7"))
            .await;

        let message = expect_failure_class(outcome, FailureClass::Permanent);
        assert!(
            message.contains("agent made no change to the pull request head"),
            "unexpected message: {message}"
        );
        assert_eq!(
            git_output([
                "-C",
                path_str(&fixture.origin),
                "rev-parse",
                &format!("refs/heads/{branch}"),
            ]),
            assigned_head
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
