// SPDX-License-Identifier: MPL-2.0

//! Budget-exhaustion retry acceptance coverage for writable native agents.

use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use jig_core::{Reply, Script, StopReason, Turn};
use temper_forge_model::{Forge, Issue};
use temper_protocol_agent::{SubmitForPrRequest, SubmitForPrResponse};
use temper_protocol_worker::{FailureClass, ResultStatus};
use temper_testing::real_stack::{
    HermeticIssueSpec, HermeticRealStack, HermeticRealStackBuilder, PausePoint,
};
use temper_worker::AgentSessionStore;

const TRACKED_CONTENT: &str = "dirty tracked work from exhausted attempt\n";
const UNTRACKED_CONTENT: &str = "dirty untracked work from exhausted attempt\n";
const UNTRACKED_PATH: &str = "BUDGET-EXHAUSTED.txt";

#[test]
fn budget_exhaustion_preserves_dirty_work_and_session_for_redispatch() {
    temper_engine_io::block_on_with(|cx, handle| async move {
        let model_calls = Arc::new(AtomicUsize::new(0));
        let submit_calls = Arc::new(Mutex::new(Vec::<SubmitForPrRequest>::new()));
        let calls_for_host = Arc::clone(&submit_calls);
        let host: temper_agent::SubmitForPrHost = Arc::new(move |request, _context, _cwd| {
            calls_for_host
                .lock()
                .expect("submit call lock")
                .push(request);
            Box::pin(std::future::ready(SubmitForPrResponse {
                accepted: true,
                message: "accepted after budget retry".to_string(),
                gates: Vec::new(),
            }))
        });

        let mut stack = HermeticRealStackBuilder::new()
            .issue(HermeticIssueSpec::ready_code(
                "Resume work after an iteration budget is exhausted",
                "Preserve tracked and untracked work, then submit it on redispatch.",
            ))
            .fake_model_script(budget_retry_script(Arc::clone(&model_calls)))
            .submit_for_pr_host(host)
            .max_iterations(2)
            .apply_grace(Duration::ZERO)
            .build(&handle)
            .await
            .expect("budget retry world builds");

        let coordination_key = format!("pr-for-code-{}", stack.issue_number().get());
        let work_branch = format!("agent/{coordination_key}");
        assert_eq!(
            stack
                .enqueue_scanned_role_work(stack.clock().now())
                .await
                .expect("initial budget-limited work enqueues"),
            1
        );
        stack.start_worker(&handle);

        let exhausted = stack
            .await_worker_result(&cx, Duration::from_secs(20))
            .await
            .expect("budget-exhausted attempt publishes a result");
        assert_eq!(exhausted.status, ResultStatus::Failure);
        assert!(exhausted.repos.is_empty());
        let failure = exhausted
            .failure
            .as_ref()
            .expect("budget exhaustion carries failure details");
        assert_eq!(failure.class, FailureClass::Transient);
        assert!(
            failure.message.contains("budget_exhausted"),
            "typed terminal reason must survive the worker boundary: {}",
            failure.message
        );
        assert!(
            !failure
                .message
                .contains("requires an accepted submit_for_pr"),
            "the undispatched submit must not become a completed-run gate failure: {}",
            failure.message
        );
        assert!(
            submit_calls.lock().expect("submit call lock").is_empty(),
            "the submit co-emitted after the final budgeted tool round must not execute"
        );
        assert_eq!(
            sorted_branches(&stack),
            vec!["main".to_string()],
            "the exhausted attempt must not push its work branch"
        );
        assert!(
            stack
                .pull_requests()
                .await
                .expect("pull request inventory")
                .is_empty(),
            "the exhausted attempt must not create a PR"
        );

        let released_issue = wait_for_released_issue(&stack, &cx).await;
        assert!(released_issue.labels.iter().any(|label| label == "ready"));
        assert!(
            !released_issue
                .labels
                .iter()
                .any(|label| label == "in-progress")
        );
        assert!(released_issue.assignees.is_empty());
        assert_no_permanent_failure_artifacts(&stack, &released_issue).await;

        let checkout = stack
            .workspace_checkout(stack.primary_repo_path())
            .expect("exhausted checkout remains available");
        assert_dirty_budget_work(&checkout);
        assert_eq!(
            stack
                .origin_file(stack.primary_repo_path(), "main", "README.md")
                .expect("default branch remains readable"),
            "# service\n",
            "dirty tracked work must remain local before redispatch"
        );
        assert!(
            stack
                .origin_file(stack.primary_repo_path(), "main", UNTRACKED_PATH)
                .is_err(),
            "dirty untracked work must remain local before redispatch"
        );

        let session_store = AgentSessionStore::for_workspace_root(
            stack.workspace_root(),
            "engineer",
            &coordination_key,
        )
        .expect("coordination-key session store");
        let exhausted_session = session_store
            .load_sync()
            .expect("load exhausted session state")
            .expect("exhausted attempt persisted a session");
        assert!(!exhausted_session.session_id.is_empty());
        assert_eq!(
            stack.observed_agent_sessions(),
            vec![Some(exhausted_session.clone())],
            "the first runner invocation must receive the persisted session state"
        );

        let retry_started = stack.pause_hooks().arm(PausePoint::AgentSessionStarted);
        assert_eq!(
            stack
                .enqueue_scanned_role_work(stack.clock().now())
                .await
                .expect("retry release permits redispatch"),
            1
        );
        let retry_started = skein::time::timeout(
            temper_engine_io::runtime::timer_now(&cx),
            Duration::from_secs(20),
            Box::pin(retry_started.arrived()),
        )
        .await
        .expect("redispatched agent session starts");

        assert_eq!(
            stack
                .workspace_checkout(stack.primary_repo_path())
                .expect("redispatch reuses checkout"),
            checkout,
            "redispatch must attach to the same coordination-key checkout"
        );
        assert_dirty_budget_work(&checkout);
        assert_eq!(
            session_store
                .load_sync()
                .expect("reload session before retry model call"),
            Some(exhausted_session.clone())
        );
        assert_eq!(
            stack.observed_agent_sessions(),
            vec![
                Some(exhausted_session.clone()),
                Some(exhausted_session.clone()),
            ],
            "redispatch must load the same session ID and extension state"
        );
        retry_started.release();

        let success = loop {
            let result = stack
                .await_worker_result(&cx, Duration::from_secs(20))
                .await
                .expect("redispatched attempt succeeds");
            if result.status == ResultStatus::Success {
                break result;
            }
        };
        assert_eq!(success.status, ResultStatus::Success);
        assert_eq!(success.repos.len(), 1);
        assert_eq!(success.repos[0].branch.name, work_branch);
        {
            let submit_calls = submit_calls.lock().expect("submit call lock");
            assert_eq!(submit_calls.len(), 1);
            assert_eq!(
                submit_calls[0].summary.as_deref(),
                Some("submit preserved budget work"),
                "the single accepted submit must execute on the retry"
            );
            assert_eq!(submit_calls[0].correlation_key, coordination_key);
            assert_eq!(submit_calls[0].role, "engineer");
            assert_eq!(submit_calls[0].action, "open_pr");
        }

        let pulls = stack
            .wait_for_pull_request_count(&cx, 1, Duration::from_secs(10))
            .await
            .expect("one implementation PR appears");
        assert_eq!(pulls.len(), 1);
        assert_eq!(pulls[0].source.branch, work_branch);
        assert!(pulls[0].body.contains("Preserved exhausted-attempt work."));
        assert_eq!(
            sorted_branches(&stack),
            vec![work_branch.clone(), "main".to_string()],
            "only the single successful work branch may be pushed"
        );
        assert_eq!(
            stack
                .origin_file(stack.primary_repo_path(), &work_branch, "README.md")
                .expect("tracked exhausted-attempt work was pushed"),
            TRACKED_CONTENT
        );
        assert_eq!(
            stack
                .origin_file(stack.primary_repo_path(), &work_branch, UNTRACKED_PATH)
                .expect("untracked exhausted-attempt work was pushed"),
            UNTRACKED_CONTENT
        );
        assert_eq!(
            stack
                .origin_log_subjects(stack.primary_repo_path(), &work_branch, 10)
                .expect("work branch history")
                .len(),
            2,
            "the work branch should contain one implementation commit over the seed"
        );

        let publications = stack.published_results();
        assert_eq!(publications.len(), 2);
        assert_eq!(
            publications
                .iter()
                .filter(|result| {
                    result.failure.as_ref().map(|failure| failure.class)
                        == Some(FailureClass::Transient)
                })
                .count(),
            1
        );
        assert_eq!(
            publications
                .iter()
                .filter(|result| result.status == ResultStatus::Success)
                .count(),
            1,
            "only the redispatched attempt may publish success"
        );
        assert_eq!(model_calls.load(Ordering::SeqCst), 5);
        assert_eq!(
            session_store.load_sync().expect("load final session state"),
            Some(exhausted_session)
        );
        assert_no_permanent_failure_artifacts(&stack, &current_issue(&stack).await).await;
        stack.crash_worker().await;
    });
}

fn budget_retry_script(model_calls: Arc<AtomicUsize>) -> Script {
    Script::rule(
        move |view| match model_calls.fetch_add(1, Ordering::SeqCst) {
            0 => Reply {
                turns: vec![
                    Turn::ToolCall {
                        id: "write-budget-tracked".to_string(),
                        name: "write".to_string(),
                        args: serde_json::json!({
                            "path": "service/README.md",
                            "content": TRACKED_CONTENT,
                        }),
                    },
                    Turn::ToolCall {
                        id: "write-budget-untracked".to_string(),
                        name: "write".to_string(),
                        args: serde_json::json!({
                            "path": format!("service/{UNTRACKED_PATH}"),
                            "content": UNTRACKED_CONTENT,
                        }),
                    },
                ],
                usage: Default::default(),
                stop: StopReason::ToolCalls,
            },
            1 => Reply {
                turns: vec![Turn::ToolCall {
                    id: "read-budget-tracked".to_string(),
                    name: "read".to_string(),
                    args: serde_json::json!({ "path": "service/README.md" }),
                }],
                usage: Default::default(),
                stop: StopReason::ToolCalls,
            },
            2 => Reply {
                turns: vec![
                    Turn::Text(
                        serde_json::json!({
                            "title": "Undispatchable exhausted result",
                            "body": "This parseable result must not be accepted.",
                            "summary": "budget response must not complete"
                        })
                        .to_string(),
                    ),
                    Turn::ToolCall {
                        id: "undispatchable-budget-submit".to_string(),
                        name: "submit_for_pr".to_string(),
                        args: serde_json::json!({ "summary": "must not execute" }),
                    },
                ],
                usage: Default::default(),
                stop: StopReason::ToolCalls,
            },
            3 => Reply {
                turns: vec![Turn::ToolCall {
                    id: "retry-budget-submit".to_string(),
                    name: "submit_for_pr".to_string(),
                    args: serde_json::json!({ "summary": "submit preserved budget work" }),
                }],
                usage: Default::default(),
                stop: StopReason::ToolCalls,
            },
            4 => Reply::text(
                serde_json::json!({
                    "title": "Preserve dirty work across budget retry",
                    "body": "# Implementation report\nPreserved exhausted-attempt work.",
                    "summary": "Preserved exhausted-attempt work."
                })
                .to_string(),
            ),
            extra => panic!(
                "unexpected model call {extra} with {} prior tool result(s)",
                view.prior_tool_results
            ),
        },
    )
}

fn assert_dirty_budget_work(checkout: &Path) {
    assert_eq!(
        std::fs::read_to_string(checkout.join("README.md")).expect("tracked dirty work"),
        TRACKED_CONTENT
    );
    assert_eq!(
        std::fs::read_to_string(checkout.join(UNTRACKED_PATH)).expect("untracked dirty work"),
        UNTRACKED_CONTENT
    );
    let output = Command::new("git")
        .args([
            "-C",
            checkout.to_str().expect("UTF-8 checkout"),
            "status",
            "--short",
        ])
        .output()
        .expect("run git status");
    assert!(output.status.success(), "git status failed: {output:?}");
    let status = String::from_utf8(output.stdout).expect("git status is UTF-8");
    let lines = status.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 2, "unexpected dirty checkout status: {status}");
    assert!(
        lines.contains(&" M README.md"),
        "tracked edit missing: {status}"
    );
    assert!(
        lines.contains(&"?? BUDGET-EXHAUSTED.txt"),
        "untracked edit missing: {status}"
    );
}

fn sorted_branches(stack: &HermeticRealStack) -> Vec<String> {
    let mut branches = stack
        .origin_branches(stack.primary_repo_path())
        .expect("origin branch inventory");
    branches.sort();
    branches
}

async fn wait_for_released_issue(stack: &HermeticRealStack, cx: &skein::cx::Cx) -> Issue {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let issue = current_issue(stack).await;
        if issue.labels.iter().any(|label| label == "ready")
            && !issue.labels.iter().any(|label| label == "in-progress")
        {
            return issue;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for retryable result application: {:?}",
            issue.labels
        );
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

async fn assert_no_permanent_failure_artifacts(stack: &HermeticRealStack, issue: &Issue) {
    assert!(
        !issue.labels.iter().any(|label| label == "needs-human"),
        "budget exhaustion must not produce a needs-human outcome: {:?}",
        issue.labels
    );
    assert!(
        !issue.body.contains("Temper run ledger"),
        "budget exhaustion must not append a run ledger: {}",
        issue.body
    );
    let comments = stack
        .forge()
        .list_issue_comments(&issue.id)
        .await
        .expect("issue comments list");
    assert!(
        comments.is_empty(),
        "budget exhaustion must not leave a submit-gate/failure audit: {:?}",
        comments
            .iter()
            .map(|comment| comment.body.as_str())
            .collect::<Vec<_>>()
    );
}
