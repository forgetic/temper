use std::time::Duration;

use temper_protocol_worker::{RepoOutcome, ResultStatus};
use temper_testing::real_stack::{
    FakeModelResponse, FakeModelWrite, HermeticIssueSpec, HermeticRealStackBuilder,
    HermeticRepoSpec,
};

#[test]
fn hermetic_real_stack_multi_repo_product_diff_opens_one_pr_per_writable_repo() {
    temper_engine_io::block_on_with(|cx, handle| async move {
        let mut stack = HermeticRealStackBuilder::new()
            .repo(HermeticRepoSpec::new("acme", "service"))
            .add_repo(HermeticRepoSpec::new("acme", "lib"))
            .issue(HermeticIssueSpec::ready_code(
                "Coordinate service and library notes",
                multi_repo_workspace_issue_body(),
            ))
            .fake_model_response(FakeModelResponse::write_files(
                [
                    FakeModelWrite::new("service/SERVICE_NOTES.md", "service notes\n"),
                    FakeModelWrite::new("lib/LIB_NOTES.md", "library notes\n"),
                ],
                "Added coordinated notes to service and lib.",
            ))
            .build(&handle)
            .await
            .expect("multi-repo hermetic real stack builds");

        let run = stack
            .run_open_pr_job(&cx, &handle)
            .await
            .expect("real worker/daemon/native-agent path completes for multi-repo job");

        assert_eq!(run.enqueued_jobs, 1);
        assert_eq!(run.job_result.status, ResultStatus::Success);
        assert_eq!(run.job_result.repos.len(), 2);
        let service = repo_outcome(&run.job_result.repos, "acme/service");
        let lib = repo_outcome(&run.job_result.repos, "acme/lib");
        assert_eq!(
            service.branch.name, lib.branch.name,
            "coordinated writable repos should share the work branch"
        );
        assert_eq!(
            stack
                .origin_rev("acme/service", &service.branch.name)
                .expect("service branch pushed to local origin"),
            service.branch.head_sha
        );
        assert_eq!(
            stack
                .origin_rev("acme/lib", &lib.branch.name)
                .expect("lib branch pushed to local origin"),
            lib.branch.head_sha
        );
        assert_eq!(
            stack
                .origin_file("acme/service", &service.branch.name, "SERVICE_NOTES.md")
                .expect("service product file exists on pushed branch"),
            "service notes\n"
        );
        assert_eq!(
            stack
                .origin_file("acme/lib", &lib.branch.name, "LIB_NOTES.md")
                .expect("lib product file exists on pushed branch"),
            "library notes\n"
        );

        let service_pulls = stack
            .wait_for_pull_request_count_for_repo(&cx, "acme/service", 1, Duration::from_secs(5))
            .await
            .expect("primary service repo gets one PR");
        let lib_pulls = stack
            .wait_for_pull_request_count_for_repo(&cx, "acme/lib", 1, Duration::from_secs(5))
            .await
            .expect("secondary lib repo gets one PR");
        let service_pull = &service_pulls[0];
        let lib_pull = &lib_pulls[0];
        assert_eq!(service_pull.source.branch, service.branch.name);
        assert_eq!(lib_pull.source.branch, lib.branch.name);
        assert!(
            service_pull
                .body
                .contains("Added coordinated notes to service and lib."),
            "primary PR body should include the agent summary: {}",
            service_pull.body
        );
        assert!(
            lib_pull
                .body
                .contains("Added coordinated notes to service and lib."),
            "secondary PR body should include the agent summary: {}",
            lib_pull.body
        );

        let primary_repo_id = stack
            .repo_id("acme/service")
            .expect("primary repo id is exposed")
            .clone();
        let service_meta = temper_workflow::parse_metadata_block(&service_pull.body)
            .expect("service PR metadata parses")
            .expect("service PR has workflow metadata");
        assert_eq!(
            service_meta.parents,
            vec![temper_workflow::ArtifactRef::same_repo(
                stack.issue_number()
            )]
        );
        let expected_correlation_key = format!("pr-for-code-{}", stack.issue_number().get());
        assert_eq!(
            service_meta.correlation_key.as_deref(),
            Some(expected_correlation_key.as_str())
        );

        let lib_meta = temper_workflow::parse_metadata_block(&lib_pull.body)
            .expect("lib PR metadata parses")
            .expect("lib PR has workflow metadata");
        assert_eq!(
            lib_meta.parents,
            vec![temper_workflow::ArtifactRef::in_repo(
                primary_repo_id.clone(),
                stack.issue_number()
            )]
        );
        assert_eq!(
            lib_meta.dependencies,
            vec![temper_workflow::ArtifactRef::in_repo(
                primary_repo_id,
                service_pull.number
            )]
        );
    });
}

fn multi_repo_workspace_issue_body() -> &'static str {
    r#"Add a small notes file to the service repo and the library repo.

<!-- temper:workspace
{"repos":[{"repo":"acme/service","access":"writable"},{"repo":"acme/lib","access":"writable","depends_on":["acme/service"]}]}
-->"#
}

fn repo_outcome<'a>(repos: &'a [RepoOutcome], repo: &str) -> &'a RepoOutcome {
    repos
        .iter()
        .find(|outcome| outcome.repo == repo)
        .unwrap_or_else(|| panic!("missing repo outcome for {repo}: {repos:?}"))
}
