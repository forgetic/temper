use super::support::target_branch::*;
use super::support::*;

#[test]
fn missing_target_branch_is_created_from_default_before_work_branch_checkout() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let target_branch = "feature/plan-centric-delivery";
        let main_head = branch_head(&fixture, "acme/service", "main");
        let agent = AgentBehavior::Success.runner();
        let executor = fixture.executor(agent.clone(), true);

        let outcome = executor
            .execute(single_repo_assign(
                "pr-for-code-155",
                "agent/pr-for-code-155",
                "main",
                target_branch,
            ))
            .await;

        let (branch_name, _head_sha, summary) = expect_success(outcome);
        assert_eq!(branch_name, "agent/pr-for-code-155");
        assert_eq!(summary.as_deref(), Some("did the work"));
        assert_eq!(
            branch_head(&fixture, "acme/service", target_branch),
            main_head,
            "new target branch is materialized from the default branch"
        );
        assert_eq!(
            agent.observed_head_sha(),
            main_head,
            "work branch starts from the freshly created target branch"
        );
    });
}

#[test]
fn existing_target_branch_is_reused_without_resetting_it() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let target_branch = "feature/existing-plan-branch";
        let target_head = seed_feature_branch(&fixture, "acme/service", target_branch);
        let agent = AgentBehavior::Success.runner();
        let executor = fixture.executor(agent.clone(), true);

        let outcome = executor
            .execute(single_repo_assign(
                "pr-for-code-155-existing",
                "agent/pr-for-code-155-existing",
                "main",
                target_branch,
            ))
            .await;

        let (branch_name, _head_sha, summary) = expect_success(outcome);
        assert_eq!(branch_name, "agent/pr-for-code-155-existing");
        assert_eq!(summary.as_deref(), Some("did the work"));
        assert_eq!(
            branch_head(&fixture, "acme/service", target_branch),
            target_head,
            "existing target branch must not be reset to the default branch"
        );
        assert_eq!(
            agent.observed_head_sha(),
            target_head,
            "work branch starts from the existing target branch head"
        );
    });
}

#[test]
fn each_writable_repo_materializes_target_branch_independently() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        seed_repo_from_service_main(&fixture, "acme/lib");
        let target_branch = "feature/coordinated-target";
        let service_main = branch_head(&fixture, "acme/service", "main");
        let lib_main = branch_head(&fixture, "acme/lib", "main");
        let executor = fixture.executor(AgentBehavior::Success.runner(), true);

        let outcome = executor
            .execute(coordinated_assign(
                "pr-for-code-155-coordinated",
                "agent/pr-for-code-155-coordinated",
                target_branch,
                true,
            ))
            .await;

        let (branch_name, _head_sha, summary) = expect_success(outcome);
        assert_eq!(branch_name, "agent/pr-for-code-155-coordinated");
        assert_eq!(summary.as_deref(), Some("did the work"));
        assert_eq!(
            branch_head(&fixture, "acme/service", target_branch),
            service_main
        );
        assert_eq!(branch_head(&fixture, "acme/lib", target_branch), lib_main);
    });
}

#[test]
fn read_only_sibling_does_not_create_target_branch() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        seed_repo_from_service_main(&fixture, "acme/lib");
        let target_branch = "feature/writable-only-target";
        let service_main = branch_head(&fixture, "acme/service", "main");
        let executor = fixture.executor(AgentBehavior::Success.runner(), true);

        let outcome = executor
            .execute(coordinated_assign(
                "pr-for-code-155-readonly",
                "agent/pr-for-code-155-readonly",
                target_branch,
                false,
            ))
            .await;

        let (branch_name, _head_sha, summary) = expect_success(outcome);
        assert_eq!(branch_name, "agent/pr-for-code-155-readonly");
        assert_eq!(summary.as_deref(), Some("did the work"));
        assert_eq!(
            branch_head(&fixture, "acme/service", target_branch),
            service_main
        );
        assert_no_branch(&fixture, "acme/lib", target_branch);
    });
}

#[test]
fn missing_target_and_default_branch_reports_clear_diagnostics() {
    temper_worker_io::block_on(async {
        let fixture = Fixture::new();
        let target_branch = "feature/missing-target-and-default";
        let executor = fixture.executor(AgentBehavior::Success.runner(), true);

        let outcome = executor
            .execute(single_repo_assign(
                "pr-for-code-155-missing-default",
                "agent/pr-for-code-155-missing-default",
                "missing-default",
                target_branch,
            ))
            .await;

        let message = expect_failure_class(outcome, FailureClass::Transient);
        assert!(
            message.contains("target branch `feature/missing-target-and-default` is missing"),
            "message should name the missing target branch: {message}"
        );
        assert!(
            message.contains("default branch `missing-default` could not be fetched"),
            "message should explain the default branch fetch failure: {message}"
        );
        assert_no_origin_branch(&fixture, target_branch);
        assert_no_origin_branch(&fixture, "agent/pr-for-code-155-missing-default");
    });
}
