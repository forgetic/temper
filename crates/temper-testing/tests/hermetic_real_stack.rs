use temper_protocol_worker::ResultStatus;
use temper_testing::real_stack::{
    FakeModelResponse, HermeticIssueSpec, HermeticRealStackBuilder, HermeticRepoSpec,
};

#[test]
fn hermetic_real_stack_smoke_runs_worker_daemon_native_agent_and_opens_pr() {
    temper_engine_io::block_on_with(|cx, handle| async move {
        let mut stack = HermeticRealStackBuilder::new()
            .repo(HermeticRepoSpec::new("acme", "service"))
            .issue(HermeticIssueSpec::ready_code(
                "Create smoke artifact",
                "Add HERMETIC_SMOKE.md with the exact contents `smoke passed`.",
            ))
            .fake_model_response(FakeModelResponse::write_file(
                "service/HERMETIC_SMOKE.md",
                "smoke passed\n",
                "Added HERMETIC_SMOKE.md.",
            ))
            .build(&handle)
            .await
            .expect("hermetic real stack builds");

        let run = stack
            .run_open_pr_job(&cx, &handle)
            .await
            .expect("real worker/daemon/native-agent path completes");

        assert_eq!(run.enqueued_jobs, 1);
        assert_eq!(run.job_result.status, ResultStatus::Success);
        assert_eq!(run.job_result.repos.len(), 1);
        let outcome = &run.job_result.repos[0];
        let branch = &outcome.branch;
        assert_eq!(outcome.repo, stack.primary_repo_path());
        assert_eq!(
            stack
                .origin_rev(stack.primary_repo_path(), &branch.name)
                .expect("branch pushed to local origin"),
            branch.head_sha
        );
        assert_eq!(
            stack
                .origin_file(stack.primary_repo_path(), &branch.name, "HERMETIC_SMOKE.md")
                .expect("product file exists on pushed branch"),
            "smoke passed\n"
        );

        assert_eq!(run.pull_requests.len(), 1);
        let pull = &run.pull_requests[0];
        assert_eq!(pull.source.branch, branch.name);
        assert!(
            pull.body.contains("Added HERMETIC_SMOKE.md."),
            "PR body should include the agent summary: {}",
            pull.body
        );
    });
}
