use super::support::*;

struct StaleGuard;
struct UnavailableGuard;

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
                "merge-conflict queue no longer matches".to_string(),
            ))
        })
    }
}

impl temper_worker::PrFreshnessGuard for UnavailableGuard {
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
            Err(temper_worker::PrFreshnessFailure::Unavailable(
                "Forge freshness read timed out".to_string(),
            ))
        })
    }
}

#[test]
fn canceled_merge_conflict_repair_cleans_workspace_for_retry() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let branch = "agent/pr-for-code-7";
        let (assigned_head, _main_head) = fixture.seed_conflicting_pr_head_branch(branch);
        let stale_executor = fixture
            .executor(AgentBehavior::ResolveMainConflict.runner(), true)
            .with_pr_freshness_guard(Arc::new(StaleGuard));

        let outcome = stale_executor
            .execute(pr_merge_conflict_assign(branch, "pr-for-code-7"))
            .await;

        let message = expect_failure_class(outcome, FailureClass::Canceled);
        assert!(message.contains("merge-conflict queue no longer matches"));
        let checkout = fixture
            .workspace_root
            .join("engineer")
            .join("pr-for-code-7")
            .join("service");
        assert_eq!(
            git_output(["-C", path_str(&checkout), "status", "--porcelain"]),
            ""
        );
        assert_eq!(
            git_output(["-C", path_str(&checkout), "rev-parse", "HEAD"]),
            assigned_head,
            "canceled repair must rewind local commits to its assignment head"
        );
        let merge_head = std::process::Command::new("git")
            .args([
                "-C",
                path_str(&checkout),
                "rev-parse",
                "--verify",
                "MERGE_HEAD",
            ])
            .output()
            .expect("inspect merge marker");
        assert!(
            !merge_head.status.success(),
            "canceled repair left MERGE_HEAD"
        );

        let retry = fixture.executor(AgentBehavior::ResolveMainConflict.runner(), true);
        let (branch_name, _head_sha, summary) = expect_success(
            retry
                .execute(pr_merge_conflict_assign(branch, "pr-for-code-7"))
                .await,
        );
        assert_eq!(branch_name, branch);
        assert_eq!(
            summary.as_deref(),
            Some("resolved merge conflict with main")
        );
    });
}

#[test]
fn unavailable_final_freshness_cleans_merge_workspace_for_retry() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let branch = "agent/pr-for-code-7";
        fixture.seed_conflicting_pr_head_branch(branch);
        let unavailable_executor = fixture
            .executor(AgentBehavior::ResolveMainConflict.runner(), true)
            .with_pr_freshness_guard(Arc::new(UnavailableGuard));

        let outcome = unavailable_executor
            .execute(pr_merge_conflict_assign(branch, "pr-for-code-7"))
            .await;

        let message = expect_failure_class(outcome, FailureClass::Transient);
        assert!(message.contains("Forge freshness read timed out"));
        let checkout = fixture
            .workspace_root
            .join("engineer")
            .join("pr-for-code-7")
            .join("service");
        assert_eq!(
            git_output(["-C", path_str(&checkout), "status", "--porcelain"]),
            ""
        );
        let merge_head = std::process::Command::new("git")
            .args([
                "-C",
                path_str(&checkout),
                "rev-parse",
                "--verify",
                "MERGE_HEAD",
            ])
            .output()
            .expect("inspect merge marker");
        assert!(
            !merge_head.status.success(),
            "transient freshness failure left MERGE_HEAD"
        );

        let retry = fixture.executor(AgentBehavior::ResolveMainConflict.runner(), true);
        expect_success(
            retry
                .execute(pr_merge_conflict_assign(branch, "pr-for-code-7"))
                .await,
        );
    });
}

#[test]
fn stale_pr_fix_rewinds_agent_local_commits() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let branch = "agent/pr-for-code-7";
        let assigned_head = fixture.seed_pr_head_branch(branch);
        let executor = fixture
            .executor(AgentBehavior::LocalCommit.runner(), true)
            .with_pr_freshness_guard(Arc::new(StaleGuard));

        let outcome = executor
            .execute(pr_fix_assign(branch, "pr-for-code-7"))
            .await;

        expect_failure_class(outcome, FailureClass::Canceled);
        let checkout = fixture
            .workspace_root
            .join("engineer")
            .join("pr-for-code-7")
            .join("service");
        assert_eq!(
            git_output(["-C", path_str(&checkout), "rev-parse", "HEAD"]),
            assigned_head
        );
        assert_eq!(
            git_output(["-C", path_str(&checkout), "status", "--porcelain"]),
            ""
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
