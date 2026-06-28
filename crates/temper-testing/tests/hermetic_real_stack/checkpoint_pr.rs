use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use jig_core::{Reply, Script, StopReason, Turn};
use temper_forge_model::{PullRequest, PullRequestState};
use temper_protocol_worker::ResultStatus;
use temper_testing::real_stack::{
    HermeticIssueSpec, HermeticRealStack, HermeticRealStackBuilder, HermeticRepoSpec,
};

use super::{assert_one_run_ledger, wait_for_issue_matching};

#[test]
fn hermetic_real_stack_checkpointed_product_diff_finalizes_implementation_pr() {
    temper_engine_io::block_on_with(|cx, handle| async move {
        let observed_continuation = Arc::new(AtomicUsize::new(0));
        let allow_final_summary = Arc::new(AtomicBool::new(false));
        let mut stack = HermeticRealStackBuilder::new()
            .repo(HermeticRepoSpec::new("acme", "service"))
            .issue(HermeticIssueSpec::ready_code(
                "Checkpoint delivery",
                "Add DELIVERY.md, checkpoint the product diff, and open the implementation PR.",
            ))
            .fake_model_script(checkpointed_delivery_script(
                Arc::clone(&observed_continuation),
                Arc::clone(&allow_final_summary),
            ))
            .enable_checkpoints(true)
            .build(&handle)
            .await
            .expect("hermetic real stack builds");

        let enqueued = stack
            .enqueue_scanned_role_work(super::default_now())
            .await
            .expect("checkpointed issue enqueues");
        stack.start_worker(&handle);

        let checkpoint_pull = wait_for_pull_request_matching(
            &cx,
            &stack,
            Duration::from_secs(5),
            "implementation PR opened from checkpoint progress",
            |pull| pull.body.contains("Opened from pushed checkpoint step 2"),
        )
        .await;
        assert!(
            checkpoint_pull
                .body
                .contains("final implementation summary will update this PR"),
            "checkpoint-created PR should make the pending-finalization handoff explicit:\n{}",
            checkpoint_pull.body
        );
        allow_final_summary.store(true, Ordering::SeqCst);

        let job_result = stack
            .await_worker_result(&cx, Duration::from_secs(10))
            .await
            .expect("checkpointed worker result is reported");

        assert_eq!(enqueued, 1);
        assert_eq!(job_result.status, ResultStatus::Success);
        assert_eq!(job_result.repos.len(), 1);
        assert_eq!(
            observed_continuation.load(Ordering::SeqCst),
            1,
            "fake model should have completed the post-checkpoint continuation"
        );

        let outcome = &job_result.repos[0];
        assert_eq!(outcome.repo, stack.primary_repo_path());
        let branch = &outcome.branch;
        assert_eq!(
            stack
                .origin_rev(stack.primary_repo_path(), &branch.name)
                .expect("checkpoint branch pushed to local origin"),
            branch.head_sha
        );
        assert_eq!(
            stack
                .origin_file(stack.primary_repo_path(), &branch.name, "DELIVERY.md")
                .expect("checkpointed product file exists on pushed branch"),
            "delivered by hermetic checkpoint\n"
        );
        assert_eq!(
            stack
                .origin_log_subjects(stack.primary_repo_path(), &branch.name, 1)
                .expect("checkpoint branch log is readable"),
            vec!["checkpoint(step 2): Create delivery file".to_string()],
            "the final PR head must be the checkpoint commit, not a late finalization commit"
        );

        let pull = wait_for_pull_request_matching(
            &cx,
            &stack,
            Duration::from_secs(5),
            "final implementation PR body",
            |pull| {
                pull.body
                    .contains("Summary: Created DELIVERY.md via checkpoint-only flow.")
            },
        )
        .await;
        assert_eq!(pull.number, checkpoint_pull.number);
        assert_eq!(pull.state, PullRequestState::Open);
        assert_eq!(
            pull.title,
            format!(
                "Implement #{}: Checkpoint delivery",
                stack.issue_number().get()
            )
        );
        assert_eq!(pull.source.branch, branch.name);
        assert_eq!(pull.target.branch, "main");
        assert!(
            pull.labels.iter().any(|label| label == "implementation"),
            "implementation PR should carry workflow label(s): {:?}",
            pull.labels
        );
        assert!(
            !pull.body.contains("Implementation plan") && !pull.body.contains("- [ ]"),
            "final PR body should be the product-diff handoff, not a model-authored checklist:\n{}",
            pull.body
        );
        let metadata = temper_workflow::parse_metadata_block(&pull.body)
            .expect("PR metadata parses")
            .expect("PR has workflow metadata");
        let expected_correlation_key = format!("pr-for-code-{}", stack.issue_number().get());
        assert_eq!(
            metadata.correlation_key.as_deref(),
            Some(expected_correlation_key.as_str())
        );
        assert_eq!(
            metadata.parents,
            vec![temper_workflow::ArtifactRef::same_repo(
                stack.issue_number()
            )]
        );
        assert_eq!(
            metadata.kind.as_ref().map(|kind| kind.as_str()),
            Some("implementation_pr")
        );

        let issue = wait_for_issue_matching(
            &cx,
            &stack,
            Duration::from_secs(5),
            "source issue finalized to implementation PR",
            |issue| {
                issue
                    .body
                    .contains(&format!("continued in PR #{}", pull.number.get()))
            },
        )
        .await;
        assert_one_run_ledger(
            &issue.body,
            &format!("pr-for-code-{}", stack.issue_number().get()),
        );
    });
}

fn checkpointed_delivery_script(
    observed_continuation: Arc<AtomicUsize>,
    allow_final_summary: Arc<AtomicBool>,
) -> Script {
    let turn_index = Arc::new(AtomicUsize::new(0));
    Script::rule(
        move |_view| match turn_index.fetch_add(1, Ordering::SeqCst) {
            0 => Reply {
                turns: vec![Turn::ToolCall {
                    id: "call_write_delivery".to_string(),
                    name: "write".to_string(),
                    args: serde_json::json!({
                        "path": "service/DELIVERY.md",
                        "content": "delivered by hermetic checkpoint\n"
                    }),
                }],
                usage: Default::default(),
                stop: StopReason::ToolCalls,
            },
            1 => Reply {
                turns: vec![Turn::ToolCall {
                    id: "call_checkpoint_delivery".to_string(),
                    name: "checkpoint".to_string(),
                    args: serde_json::json!({ "label": "Create delivery file" }),
                }],
                usage: Default::default(),
                stop: StopReason::ToolCalls,
            },
            _ => {
                let deadline = Instant::now() + Duration::from_secs(10);
                while !allow_final_summary.load(Ordering::SeqCst) && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(10));
                }
                observed_continuation.fetch_add(1, Ordering::SeqCst);
                Reply::text(r#"{"summary":"Created DELIVERY.md via checkpoint-only flow."}"#)
            }
        },
    )
}

async fn wait_for_pull_request_matching(
    cx: &skein::cx::Cx,
    stack: &HermeticRealStack,
    timeout: Duration,
    description: &str,
    predicate: impl Fn(&PullRequest) -> bool,
) -> PullRequest {
    let deadline = Instant::now() + timeout;
    loop {
        let pulls = stack.pull_requests().await.expect("pull requests list");
        if let Some(pull) = pulls.iter().find(|pull| predicate(pull)) {
            return pull.clone();
        }
        if Instant::now() >= deadline {
            panic!(
                "timed out after {timeout:?} waiting for {description}; pull bodies:\n{}",
                pulls
                    .iter()
                    .map(|pull| format!("#{} {}\n{}", pull.number, pull.title, pull.body))
                    .collect::<Vec<_>>()
                    .join("\n---\n")
            );
        }
        temper_engine_io::runtime::sleep_for(cx, Duration::from_millis(10)).await;
    }
}
