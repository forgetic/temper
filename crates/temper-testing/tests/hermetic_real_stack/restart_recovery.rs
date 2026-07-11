use chrono::Duration;
use temper_forge_model::Forge;
use temper_protocol_worker::ResultStatus;
use temper_testing::real_stack::{
    FakeModelResponse, HermeticIssueSpec, HermeticRealStackBuilder, HermeticRepoSpec, PauseHooks,
    PausePoint,
};
use temper_workflow::parse_metadata_block;

#[test]
fn named_pause_is_channel_driven_and_one_shot() {
    temper_engine_io::block_on_with(|_cx, handle| async move {
        let hooks = PauseHooks::default();
        let permit = hooks.arm(PausePoint::ChildWired);
        let component_hooks = hooks.clone();
        let (done_tx, done_rx) = temper_engine_io::oneshot();
        handle.spawn(async move {
            component_hooks.reach(PausePoint::ChildWired).await;
            done_tx.send(());
        });

        let reached = permit.arrived().await;
        assert_eq!(reached.point(), PausePoint::ChildWired);
        reached.release();
        assert_eq!(done_rx.recv().await, Some(()));

        // The same point is not sticky: reaching it again needs no release.
        hooks.reach(PausePoint::ChildWired).await;
    });
}

#[test]
fn crash_after_claim_commit_converges_orphan_before_redispatch() {
    temper_engine_io::block_on_with(|cx, handle| async move {
        let mut stack = HermeticRealStackBuilder::new()
            .issue(HermeticIssueSpec::ready_code(
                "Claim commit restart",
                "Add CLAIM_RECOVERED.md.\n\n<!-- temper:workflow\n{\"kind\":\"code\"}\n-->",
            ))
            .fake_model_response(FakeModelResponse::write_file(
                "service/CLAIM_RECOVERED.md",
                "recovered once\n",
                "Recovered the orphaned durable claim.",
            ))
            .build(&handle)
            .await
            .expect("durable world builds");

        assert_eq!(
            stack
                .enqueue_scanned_role_work(stack.clock().now())
                .await
                .expect("source enqueues"),
            1
        );
        let claim_pause = stack
            .pause_hooks()
            .arm(PausePoint::AssignmentClaimCommitted);
        stack.start_worker(&handle);
        let reached = claim_pause.arrived().await;

        let claimed = stack
            .forge()
            .get_issue_by_number(stack.primary_repo_id(), stack.issue_number())
            .await
            .expect("issue lookup")
            .expect("source issue");
        let claimed_metadata = parse_metadata_block(&claimed.body)
            .expect("metadata parses")
            .expect("metadata exists");
        assert!(claimed_metadata.assignment.is_some());
        assert!(claimed_metadata.lease.is_some());

        // The worker machine never receives Assign: cancellation/join happens
        // while its transport task is held at the named post-CAS hook.
        stack.crash_worker().await;
        reached.release();
        stack.replace_daemon(&handle).await;
        let orphaned = stack.open_recovery_barrier().await;
        assert_eq!(orphaned.len(), 1);

        let rolled_back = stack
            .forge()
            .get_issue_by_number(stack.primary_repo_id(), stack.issue_number())
            .await
            .expect("issue lookup")
            .expect("source issue");
        let metadata = parse_metadata_block(&rolled_back.body)
            .expect("metadata parses")
            .expect("metadata exists");
        assert!(metadata.assignment.is_none());
        assert!(metadata.lease.is_none());
        assert!(rolled_back.labels.iter().any(|label| label == "ready"));
        assert!(
            !rolled_back
                .labels
                .iter()
                .any(|label| label == "in-progress")
        );

        let run = stack
            .run_open_pr_job(&cx, &handle)
            .await
            .expect("orphan is redispatched through recreated components");
        assert_eq!(run.job_result.status, ResultStatus::Success);
        assert_eq!(run.pull_requests.len(), 1);
        stack.crash_worker().await;
    });
}

#[test]
fn daemon_and_worker_replacement_preserve_the_durable_world_and_converge() {
    temper_engine_io::block_on_with(|cx, handle| async move {
        let mut stack = HermeticRealStackBuilder::new()
            .repo(HermeticRepoSpec::new("acme", "service"))
            .issue(HermeticIssueSpec::ready_code(
                "Restart-safe delivery",
                "Add RESTARTED.md with deterministic content.",
            ))
            .fake_model_response(FakeModelResponse::write_file(
                "service/RESTARTED.md",
                "survived component replacement\n",
                "Added restart convergence proof.",
            ))
            .build(&handle)
            .await
            .expect("durable world builds");

        let forge_identity = stack.forge() as *const _ as usize;
        let workspace_root = stack.workspace_root().to_path_buf();
        let origin_before = stack
            .origin_rev(stack.primary_repo_path(), "main")
            .expect("seed origin exists");
        let daemon_before = stack.daemon() as *const _ as usize;
        let advanced = stack.clock().advance(Duration::minutes(7));

        stack.replace_daemon(&handle).await;
        assert_ne!(daemon_before, stack.daemon() as *const _ as usize);
        assert_eq!(forge_identity, stack.forge() as *const _ as usize);
        assert_eq!(workspace_root, stack.workspace_root());
        assert_eq!(advanced, stack.clock().now());
        assert_eq!(
            origin_before,
            stack
                .origin_rev(stack.primary_repo_path(), "main")
                .expect("origin survives daemon replacement")
        );
        assert!(stack.open_recovery_barrier().await.is_empty());

        let run = stack
            .run_open_pr_job(&cx, &handle)
            .await
            .expect("replacement daemon and real worker converge");
        assert_eq!(run.job_result.status, ResultStatus::Success);
        assert_eq!(run.pull_requests.len(), 1);

        stack.crash_worker().await;
        stack.start_worker(&handle);
        assert_eq!(
            stack
                .enqueue_scanned_role_work(stack.clock().now())
                .await
                .expect("idle scan succeeds"),
            0,
            "completed source must not become dispatchable after worker replacement"
        );
        stack.crash_worker().await;

        let issue = stack
            .forge()
            .get_issue_by_number(stack.primary_repo_id(), stack.issue_number())
            .await
            .expect("issue lookup")
            .expect("source issue");
        let metadata = parse_metadata_block(&issue.body)
            .expect("metadata parses")
            .expect("workflow metadata remains");
        assert!(metadata.assignment.is_none(), "no stale assignment remains");
        assert!(metadata.lease.is_none(), "no stale lease remains");
        assert_eq!(
            stack
                .pull_requests()
                .await
                .expect("pull request list")
                .len(),
            1,
            "component replacement must not duplicate the PR"
        );
    });
}
