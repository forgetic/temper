use super::support::*;

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
