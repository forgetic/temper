// SPDX-License-Identifier: MPL-2.0

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use temper_forge_model::{Forge, IssueQuery, UpdateIssue};
use temper_protocol_worker::{
    ContextOutcome, FetchContext, ForgeContextErrorCode, ForgeContextOperation,
    ForgeGetItemOperation, JobChild, JobResult, ReleaseDisposition, ResultStatus,
    WORKER_PROTOCOL_VERSION, WorkerProtocolMessage,
};
use temper_testing::real_stack::{
    HermeticIssueSpec, HermeticRealStackBuilder, HermeticRepoSpec, PausePoint, WorkerRoleSpec,
};
use temper_workflow::{DurableAssignment, ValidatedWorkflow, parse_metadata_block};

#[test]
fn label_only_feature_planning_survives_restart_and_fences_the_old_attempt() {
    temper_engine_io::block_on_with(|cx, handle| async move {
        let model_calls = Arc::new(AtomicUsize::new(0));
        let calls_for_model = Arc::clone(&model_calls);
        let builder = HermeticRealStackBuilder::new();
        #[cfg(target_os = "linux")]
        let builder = builder
            .linux_supervisor_helper(env!("CARGO_BIN_EXE_temper-real-stack-supervisor-helper"));
        let mut stack = builder
            .repo(HermeticRepoSpec::new("acme", "service"))
            .issue(
                HermeticIssueSpec::ready_code(
                    "Plan an operator-labeled feature",
                    "An ordinary operator-authored issue with no hidden workflow metadata.",
                )
                .labels(Vec::<String>::new()),
            )
            .workflow(plan_centric_feature_workflow())
            .worker_role(WorkerRoleSpec::architect())
            .fake_model_script(jig_core::Script::rule(move |_| {
                calls_for_model.fetch_add(1, Ordering::SeqCst);
                jig_core::Reply::text(
                    serde_json::json!({
                        "verdict": "needs_plan",
                        "summary": "Created one durable implementation plan.",
                        "children": [{
                            "slug": "plan",
                            "kind": "plan",
                            "title": "Plan the operator-labeled feature",
                            "body": "Plan the implementation through the feature branch.\n\n<!-- temper:workflow\n{\"kind\":\"plan\"}\n-->"
                        }]
                    })
                    .to_string(),
                )
            }))
            // Keep heartbeat reattachment out of the deterministic window: this
            // scenario intentionally exercises orphan convergence and requeue.
            .worker_heartbeat_interval(std::time::Duration::from_secs(300))
            .build(&handle)
            .await
            .expect("label-only feature world builds");

        let ordinary = stack
            .forge()
            .get_issue_by_number(stack.primary_repo_id(), stack.issue_number())
            .await
            .expect("ordinary issue lookup")
            .expect("ordinary issue exists");
        assert!(ordinary.labels.is_empty());
        assert!(
            parse_metadata_block(&ordinary.body)
                .expect("ordinary issue body parses")
                .is_none(),
            "manual intake starts without workflow metadata"
        );

        // Model the supported manual intake path: the operator changes only the
        // identifying label through Forge before the role scanner runs.
        let labeled = stack
            .forge()
            .update_issue(
                &ordinary.id,
                UpdateIssue {
                    add_labels: vec!["feature".to_string()],
                    ..UpdateIssue::default()
                },
            )
            .await
            .expect("operator adds feature label");
        assert_eq!(labeled.labels, vec!["feature"]);
        assert_eq!(labeled.body, ordinary.body);

        assert_eq!(
            stack
                .enqueue_scanned_role_work_for_role("architect", stack.clock().now())
                .await
                .expect("feature planning scan enqueues"),
            1
        );
        let old_claim_pause = stack
            .pause_hooks()
            .arm(PausePoint::AssignmentClaimCommitted);
        stack.start_worker(&handle);
        let old_claim_pause = old_claim_pause.arrived().await;

        let claimed = stack
            .forge()
            .get_issue_by_number(stack.primary_repo_id(), stack.issue_number())
            .await
            .expect("claimed issue lookup")
            .expect("claimed issue exists");
        let claimed_metadata = parse_metadata_block(&claimed.body)
            .expect("claimed metadata parses")
            .expect("claim creates durable metadata");
        assert!(
            claimed_metadata.kind.is_none(),
            "recovery must not require a redundant metadata kind"
        );
        assert!(
            claimed_metadata.lease.is_some(),
            "claim has a durable lease"
        );
        let old_assignment = claimed_metadata
            .assignment
            .clone()
            .expect("claim has a durable assignment");
        assert_eq!(
            old_assignment.role.as_ref().map(|role| role.as_str()),
            Some("architect")
        );
        assert_eq!(old_assignment.queue.as_deref(), Some("feature_planning"));
        assert_eq!(old_assignment.action.as_deref(), Some("plan_feature"));

        // Replace the daemon while publication of the claimed assignment is
        // parked at the fixture's named post-CAS pause. Inventory and orphan
        // convergence delegate to temper-engine-service, exactly like production.
        stack.replace_daemon_through_startup_recovery(&handle).await;
        let orphaned = stack.open_recovery_barrier().await;
        assert_eq!(orphaned.len(), 1);
        assert_eq!(orphaned[0].job_id, old_assignment.job_id.clone().unwrap());
        assert_eq!(orphaned[0].attempt_id, old_assignment.attempt_id);

        let requeued = stack
            .forge()
            .get_issue_by_number(stack.primary_repo_id(), stack.issue_number())
            .await
            .expect("requeued issue lookup")
            .expect("requeued issue exists");
        let requeued_metadata = parse_metadata_block(&requeued.body)
            .expect("requeued metadata parses")
            .expect("requeue preserves the workflow block");
        assert!(requeued_metadata.kind.is_none());
        assert!(requeued_metadata.assignment.is_none());
        assert!(requeued_metadata.lease.is_none());
        assert_eq!(requeued.labels, vec!["feature"]);
        assert!(!requeued.labels.iter().any(|label| label == "needs-human"));
        assert!(
            stack
                .forge()
                .list_issue_comments(&requeued.id)
                .await
                .expect("recovery comments list")
                .is_empty(),
            "safe label resolution must not publish an attention audit"
        );

        // Abruptly remove the pre-restart worker before its parked assignment
        // publication is delivered, then let ordinary scanning claim a
        // replacement.
        stack.crash_worker().await;
        old_claim_pause.release();
        assert_eq!(model_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            stack
                .enqueue_scanned_role_work_for_role("architect", stack.clock().now())
                .await
                .expect("requeued feature planning scan enqueues"),
            1
        );
        let replacement_claim_pause = stack
            .pause_hooks()
            .arm(PausePoint::AssignmentClaimCommitted);
        stack.start_worker(&handle);
        let replacement_claim_pause = replacement_claim_pause.arrived().await;

        let replacement_claim = stack
            .forge()
            .get_issue_by_number(stack.primary_repo_id(), stack.issue_number())
            .await
            .expect("replacement claim lookup")
            .expect("replacement claim exists");
        let replacement_metadata = parse_metadata_block(&replacement_claim.body)
            .expect("replacement metadata parses")
            .expect("replacement metadata exists");
        let replacement_assignment = replacement_metadata
            .assignment
            .clone()
            .expect("replacement assignment exists");
        assert_eq!(replacement_assignment.job_id, old_assignment.job_id);
        assert_ne!(
            replacement_assignment.attempt_id, old_assignment.attempt_id,
            "redispatch receives a fresh attempt fence"
        );
        assert!(replacement_metadata.lease.is_some());
        assert!(replacement_metadata.kind.is_none());

        let before_stale = replacement_claim.clone();
        let comments_before_stale = stack
            .forge()
            .list_issue_comments(&replacement_claim.id)
            .await
            .expect("comments before stale requests");

        let stale_context = FetchContext::new(
            old_assignment.worker_id.clone().unwrap(),
            old_assignment.job_id.clone().unwrap(),
            old_assignment.attempt_id.clone().unwrap(),
            ForgeContextOperation::ForgeGetItem(ForgeGetItemOperation {
                repo: stack.primary_repo_path().to_string(),
                number: stack.issue_number().get(),
                artifact_type: None,
                include_comments: true,
            }),
        );
        let Some(WorkerProtocolMessage::ContextResponse(stale_context_response)) = stack
            .daemon()
            .deliver_protocol_message(WorkerProtocolMessage::FetchContext(stale_context))
            .await
            .expect("old context request receives a protocol response")
        else {
            panic!("old context request must receive a context response")
        };
        assert_eq!(
            stale_context_response.outcome,
            ContextOutcome::Error {
                code: ForgeContextErrorCode::NotAuthorized
            }
        );

        let stale_result = needs_plan_result(&old_assignment);
        let Some(WorkerProtocolMessage::Release(stale_release)) = stack
            .daemon()
            .deliver_protocol_message(WorkerProtocolMessage::Result(stale_result))
            .await
            .expect("old result receives a protocol response")
        else {
            panic!("old result must receive a release")
        };
        assert_eq!(stale_release.disposition, ReleaseDisposition::Superseded);
        assert_eq!(stale_release.attempt_id, old_assignment.attempt_id);

        let after_stale = stack
            .forge()
            .get_issue_by_number(stack.primary_repo_id(), stack.issue_number())
            .await
            .expect("issue lookup after stale requests")
            .expect("issue remains after stale requests");
        assert_eq!(after_stale, before_stale);
        assert_eq!(
            stack
                .forge()
                .list_issue_comments(&after_stale.id)
                .await
                .expect("comments after stale requests"),
            comments_before_stale
        );
        assert_eq!(
            stack
                .forge()
                .list_issues(stack.primary_repo_id(), IssueQuery::default())
                .await
                .expect("inventory after stale requests")
                .len(),
            1,
            "the old verdict cannot create a child"
        );

        replacement_claim_pause.release();
        let replacement_attempt = replacement_assignment.attempt_id.clone();
        let accepted = loop {
            let result = stack
                .await_worker_result(&cx, std::time::Duration::from_secs(20))
                .await
                .expect("replacement architect result");
            if result.attempt_id == replacement_attempt
                && result.verdict.as_deref() == Some("needs_plan")
            {
                break result;
            }
        };
        assert_eq!(accepted.status, ResultStatus::Success);
        assert_eq!(model_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            stack
                .published_results()
                .iter()
                .filter(|result| result.verdict.as_deref() == Some("needs_plan"))
                .count(),
            1,
            "exactly one architect verdict reaches the worker publication boundary"
        );

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(45);
        let issues = loop {
            let issues = stack
                .forge()
                .list_issues(stack.primary_repo_id(), IssueQuery::default())
                .await
                .expect("final issue inventory");
            let accepted_releases = stack
                .published_releases()
                .into_iter()
                .filter(|release| {
                    release.attempt_id == replacement_attempt
                        && release.disposition == ReleaseDisposition::Accepted
                })
                .count();
            if issues.len() == 2 && accepted_releases == 1 {
                break issues;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for the accepted verdict effect; releases={:?}, pending={:?}",
                stack.published_releases(),
                stack.pending_result_count()
            );
            temper_engine_io::runtime::sleep_for(&cx, std::time::Duration::from_millis(10)).await;
        };
        assert_eq!(issues.len(), 2, "one plan child is created exactly once");
        assert_eq!(
            stack
                .published_releases()
                .iter()
                .filter(|release| release.disposition == ReleaseDisposition::Accepted)
                .count(),
            1,
            "exactly one replacement verdict is accepted and applied"
        );
        assert!(
            issues
                .iter()
                .all(|issue| !issue.labels.iter().any(|label| label == "needs-human"))
        );
        let feature = issues
            .iter()
            .find(|issue| issue.number == stack.issue_number())
            .expect("source feature remains");
        assert!(feature.labels.iter().any(|label| label == "planned"));
        let feature_metadata = parse_metadata_block(&feature.body)
            .expect("final feature metadata parses")
            .expect("final feature metadata exists");
        assert_eq!(feature_metadata.dependencies.len(), 1);
        assert!(feature_metadata.assignment.is_none());
        assert!(feature_metadata.lease.is_none());

        let plan = issues
            .iter()
            .find(|issue| issue.number != stack.issue_number())
            .expect("plan child exists");
        assert_eq!(plan.title, "Plan the operator-labeled feature");
        assert_eq!(plan.labels, vec!["plan", "ready"]);
        let plan_metadata = parse_metadata_block(&plan.body)
            .expect("plan metadata parses")
            .expect("plan metadata exists");
        assert_eq!(
            plan_metadata.kind.as_ref().map(|kind| kind.as_str()),
            Some("plan")
        );
        assert_eq!(plan_metadata.parents.len(), 1);
        assert_eq!(plan_metadata.parents[0].number, stack.issue_number());
        assert_eq!(
            plan_metadata.target_branch.as_deref(),
            Some("agent/pr-for-feature-1")
        );
        for issue in &issues {
            assert!(
                stack
                    .forge()
                    .list_issue_comments(&issue.id)
                    .await
                    .expect("final comments list")
                    .is_empty(),
                "restart and stale replay must not duplicate comments"
            );
        }
        stack.crash_worker().await;
    });
}

fn needs_plan_result(assignment: &DurableAssignment) -> JobResult {
    JobResult {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: assignment.worker_id.clone().expect("assignment worker id"),
        job_id: assignment.job_id.clone().expect("assignment job id"),
        attempt_id: assignment.attempt_id.clone(),
        status: ResultStatus::Success,
        repos: Vec::new(),
        verdict: Some("needs_plan".to_string()),
        title: None,
        body: None,
        children: vec![JobChild {
            slug: "stale-plan".to_string(),
            title: "A stale plan that must never exist".to_string(),
            body: "The old attempt must not create this child.".to_string(),
            kind: Some("plan".to_string()),
            labels: Vec::new(),
            depends_on: Vec::new(),
            target_repo: None,
        }],
        failure: None,
        summary: Some("Late pre-restart verdict.".to_string()),
        details: None,
    }
}

fn plan_centric_feature_workflow() -> ValidatedWorkflow {
    temper_workflow::parse_workflow_spec(
        "plan-centric-feature-branch/workflow.json",
        include_str!("../../../../scenarios/plan-centric-feature-branch/config/workflow.json"),
    )
    .expect("plan-centric workflow parses")
    .validate()
    .expect("plan-centric workflow validates")
}
