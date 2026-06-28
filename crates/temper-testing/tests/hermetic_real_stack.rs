use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use jig_core::{HttpError, Reply, Script, ScriptAction, StopReason, Turn};
use temper_agent_core::{StreamRetryConfig, install_stream_retry_config_override};
use temper_forge_model::{Forge, Issue};
use temper_protocol_worker::{FailureClass, JobResult, ResultStatus};
use temper_testing::real_stack::{
    FakeModelResponse, HermeticIssueSpec, HermeticRealStack, HermeticRealStackBuilder,
    HermeticRepoSpec,
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

#[test]
fn hermetic_real_stack_requeues_provider_server_error_and_later_succeeds() {
    let _retry_override = install_stream_retry_config_override(StreamRetryConfig {
        max_retries: 2,
        base_backoff: Duration::from_millis(1),
        max_backoff: Duration::from_millis(1),
    });

    temper_engine_io::block_on_with(|cx, handle| async move {
        let allow_success = Arc::new(AtomicBool::new(false));
        let provider_errors = Arc::new(AtomicUsize::new(0));
        let observed_success_continuation = Arc::new(AtomicUsize::new(0));
        let script = server_error_then_success_script(
            Arc::clone(&allow_success),
            Arc::clone(&provider_errors),
            Arc::clone(&observed_success_continuation),
        );

        let mut stack = HermeticRealStackBuilder::new()
            .repo(HermeticRepoSpec::new("acme", "service"))
            .issue(HermeticIssueSpec::ready_code(
                "Recover after provider server_error",
                "Add DELIVERY.md after Temper retries a transient model/provider server error.",
            ))
            .fake_model_script(script)
            .apply_grace(Duration::ZERO)
            .build(&handle)
            .await
            .expect("hermetic real stack builds");
        let correlation_key = format!("pr-for-code-{}", stack.issue_number().get());

        assert_eq!(
            stack
                .enqueue_scanned_role_work(default_now())
                .await
                .expect("initial role scan enqueues work"),
            1
        );
        stack.start_worker(&handle);

        let claimed = wait_for_issue_matching(
            &cx,
            &stack,
            Duration::from_secs(5),
            "source issue to be claimed by started progress",
            |issue| {
                issue.labels.iter().any(|label| label == "in-progress")
                    && !issue.labels.iter().any(|label| label == "ready")
                    && !issue.assignees.is_empty()
                    && issue.body.contains("Current status: editing")
            },
        )
        .await;
        assert_one_run_ledger(&claimed.body, &correlation_key);

        let first_result = stack
            .await_worker_result(&cx, Duration::from_secs(10))
            .await
            .expect("first worker attempt reports a provider failure");
        assert_eq!(first_result.status, ResultStatus::Failure);
        let failure = first_result
            .failure
            .as_ref()
            .expect("failed result carries failure details");
        assert_eq!(failure.class, FailureClass::Transient);
        assert!(
            failure.message.contains("server_error")
                || failure.message.contains("temporary upstream failure"),
            "failure should carry the provider server_error details: {}",
            failure.message
        );
        assert!(
            provider_errors.load(Ordering::SeqCst) >= 3,
            "fake provider should see the initial HTTP 500 plus configured stream retries"
        );

        let released = wait_for_issue_matching(
            &cx,
            &stack,
            Duration::from_secs(5),
            "transient failure retry release",
            |issue| {
                issue.labels.iter().any(|label| label == "ready")
                    && !issue.labels.iter().any(|label| label == "in-progress")
                    && issue.assignees.is_empty()
                    && issue.body.contains("Current status: queued for retry")
                    && issue
                        .body
                        .contains("Retry: released back to the ready queue")
            },
        )
        .await;
        assert_one_run_ledger(&released.body, &correlation_key);
        assert!(released.body.contains("Latest progress: step 1"));
        assert_no_human_attention(&stack).await;
        assert!(
            stack
                .pull_requests()
                .await
                .expect("pull requests list during retry")
                .is_empty(),
            "no implementation PR should exist before the retry succeeds"
        );

        allow_success.store(true, Ordering::SeqCst);
        let success_result = enqueue_until_worker_result(&cx, &mut stack, Duration::from_secs(10))
            .await
            .expect("retry scan produces a second worker result");
        assert_eq!(success_result.status, ResultStatus::Success);
        assert_eq!(success_result.repos.len(), 1);
        let outcome = &success_result.repos[0];
        assert_eq!(outcome.repo, stack.primary_repo_path());
        assert_eq!(
            stack
                .origin_file(
                    stack.primary_repo_path(),
                    &outcome.branch.name,
                    "DELIVERY.md"
                )
                .expect("retried product file exists on pushed branch"),
            "delivered after provider retry\n"
        );

        let pull_requests = stack
            .wait_for_pull_request_count(&cx, 1, Duration::from_secs(5))
            .await
            .expect("successful retry opens one implementation PR");
        let pull = &pull_requests[0];
        assert_eq!(pull.source.branch, outcome.branch.name);
        assert!(
            pull.body
                .contains("Created DELIVERY.md after provider retry."),
            "PR body should include the successful retry summary: {}",
            pull.body
        );

        let expected_handoff = format!("continued in PR #{}", pull.number.get());
        let finalized = wait_for_issue_matching(
            &cx,
            &stack,
            Duration::from_secs(5),
            "source issue ledger to finalize to the implementation PR",
            |issue| {
                issue.body.contains(&expected_handoff)
                    && !issue.body.contains("Current status: queued for retry")
            },
        )
        .await;
        assert_one_run_ledger(&finalized.body, &correlation_key);
        assert!(
            !finalized.labels.iter().any(|label| label == "needs-human"),
            "recovered issue should not be marked for human attention: {:?}",
            finalized.labels
        );
        assert_eq!(
            observed_success_continuation.load(Ordering::SeqCst),
            1,
            "fake model should have completed the successful tool-result continuation"
        );
        assert_no_human_attention(&stack).await;
    });
}

fn server_error_then_success_script(
    allow_success: Arc<AtomicBool>,
    provider_errors: Arc<AtomicUsize>,
    observed_success_continuation: Arc<AtomicUsize>,
) -> Script {
    let success_turn = Arc::new(AtomicUsize::new(0));
    Script::action_rule(move |_view| {
        if !allow_success.load(Ordering::SeqCst) {
            provider_errors.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(50));
            return ScriptAction::HttpError(HttpError::provider(
                500,
                "server_error",
                "temporary upstream failure from Jig",
            ));
        }

        let turn = success_turn.fetch_add(1, Ordering::SeqCst);
        let reply = match turn {
            0 => Reply {
                turns: vec![Turn::ToolCall {
                    id: "call_write_after_retry".to_string(),
                    name: "write".to_string(),
                    args: serde_json::json!({
                        "path": "service/DELIVERY.md",
                        "content": "delivered after provider retry\n"
                    }),
                }],
                usage: Default::default(),
                stop: StopReason::ToolCalls,
            },
            _ => {
                observed_success_continuation.fetch_add(1, Ordering::SeqCst);
                Reply::text(r#"{"summary":"Created DELIVERY.md after provider retry."}"#)
            }
        };
        ScriptAction::Reply(reply)
    })
}

async fn enqueue_until_worker_result(
    cx: &skein::cx::Cx,
    stack: &mut HermeticRealStack,
    timeout: Duration,
) -> Result<JobResult, String> {
    let deadline = Instant::now() + timeout;
    loop {
        let _ = stack.enqueue_scanned_role_work(default_now()).await?;
        match stack
            .await_worker_result(cx, Duration::from_millis(500))
            .await
        {
            Ok(result) => return Ok(result),
            Err(error) if error.contains("timed out") => {
                if Instant::now() >= deadline {
                    return Err(error);
                }
            }
            Err(error) => return Err(error),
        }
        temper_engine_io::runtime::sleep_for(cx, Duration::from_millis(25)).await;
        if Instant::now() >= deadline {
            return Err(format!(
                "timed out after {timeout:?} waiting for a worker result after re-enqueue"
            ));
        }
    }
}

async fn wait_for_issue_matching(
    cx: &skein::cx::Cx,
    stack: &HermeticRealStack,
    timeout: Duration,
    description: &str,
    predicate: impl Fn(&Issue) -> bool,
) -> Issue {
    let deadline = Instant::now() + timeout;
    loop {
        let issue = current_issue(stack).await;
        if predicate(&issue) {
            return issue;
        }
        if Instant::now() >= deadline {
            panic!(
                "timed out after {timeout:?} waiting for {description}; labels={:?} assignees={:?}\n{}",
                issue.labels, issue.assignees, issue.body
            );
        }
        temper_engine_io::runtime::sleep_for(cx, Duration::from_millis(10)).await;
    }
}

async fn current_issue(stack: &HermeticRealStack) -> Issue {
    stack
        .forge()
        .get_issue_by_number(stack.primary_repo_id(), stack.issue_number())
        .await
        .expect("issue lookup succeeds")
        .expect("source issue exists")
}

async fn assert_no_human_attention(stack: &HermeticRealStack) {
    let issue = current_issue(stack).await;
    assert!(
        !issue.labels.iter().any(|label| label == "needs-human"),
        "source issue should not have human-attention labels after transient provider failure: {:?}",
        issue.labels
    );
    let comments = stack
        .forge()
        .list_issue_comments(&issue.id)
        .await
        .expect("issue comments list");
    assert!(
        comments.is_empty(),
        "transient provider failure should not leave audit comments: {:?}",
        comments
            .iter()
            .map(|comment| comment.body.as_str())
            .collect::<Vec<_>>()
    );
}

fn assert_one_run_ledger(body: &str, correlation_key: &str) {
    let marker = format!("<!-- temper-run-ledger correlation_key={correlation_key} -->");
    assert_eq!(
        body.matches(&marker).count(),
        1,
        "expected exactly one run ledger marker in issue body: {body}"
    );
    assert_eq!(
        body.matches("<!-- /temper-run-ledger -->").count(),
        1,
        "expected exactly one run ledger end marker in issue body: {body}"
    );
}

fn default_now() -> DateTime<Utc> {
    "2026-05-29T00:00:00Z"
        .parse()
        .expect("default hermetic timestamp parses")
}
