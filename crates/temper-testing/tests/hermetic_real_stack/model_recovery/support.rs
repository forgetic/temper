// SPDX-License-Identifier: MPL-2.0

use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use jig_core::{HttpError, Reply, Script, ScriptAction, StopReason, Turn};
use temper_forge_model::{Forge, Issue};
use temper_protocol_activity::ModelFailureCategoryV1;
use temper_protocol_worker::{
    FailureClass, JobResult, ReleaseDisposition, ResultStatus, SessionRecoveryActionV1,
    WorkerProtocolMessage,
};
use temper_testing::real_stack::{HermeticRealStack, HermeticRealStackBuilder};
use temper_worker::AgentSessionLedger;
use temper_workflow::parse_metadata_block;

pub(super) const TRACKED_CONTENT: &str = "tracked work from the consumed model session\n";
pub(super) const UNTRACKED_CONTENT: &str = "untracked work from the consumed model session\n";
pub(super) const UNTRACKED_PATH: &str = "MODEL-RECOVERY.txt";
const PROVIDER_CODE: &str = "invalid_api_key";

pub(super) fn model_recovery_builder() -> HermeticRealStackBuilder {
    let builder = HermeticRealStackBuilder::new();
    #[cfg(target_os = "linux")]
    let builder =
        builder.linux_supervisor_helper(env!("CARGO_BIN_EXE_temper-real-stack-supervisor-helper"));
    builder
}

pub(super) fn recovery_success_script(model_calls: Arc<AtomicUsize>) -> Script {
    Script::action_rule(
        move |view| match model_calls.fetch_add(1, Ordering::SeqCst) {
            0 => ScriptAction::Reply(write_predecessor_work()),
            1 => non_retryable_model_failure(),
            2 => ScriptAction::Reply(read_predecessor_work()),
            3 => ScriptAction::Reply(Reply {
                turns: vec![Turn::ToolCall {
                    id: "submit-recovered-work".to_string(),
                    name: "submit_for_pr".to_string(),
                    args: serde_json::json!({ "summary": "submit recovered predecessor work" }),
                }],
                usage: Default::default(),
                stop: StopReason::ToolCalls,
            }),
            4 => ScriptAction::Reply(Reply::text(
                serde_json::json!({
                    "title": "Recover product after bounded model failure",
                    "body": "# Implementation report\nRecovered predecessor workspace changes.",
                    "summary": "Recovered predecessor workspace changes."
                })
                .to_string(),
            )),
            extra => panic!(
                "unexpected success model call {extra} with {} prior tool results",
                view.prior_tool_results
            ),
        },
    )
}

pub(super) fn recovery_exhaustion_script(model_calls: Arc<AtomicUsize>) -> Script {
    Script::action_rule(
        move |view| match model_calls.fetch_add(1, Ordering::SeqCst) {
            0 => ScriptAction::Reply(write_predecessor_work()),
            1 => non_retryable_model_failure(),
            2 => ScriptAction::Reply(read_predecessor_work()),
            3 => non_retryable_model_failure(),
            extra => panic!(
                "unexpected exhaustion model call {extra} with {} prior tool results",
                view.prior_tool_results
            ),
        },
    )
}

fn write_predecessor_work() -> Reply {
    Reply {
        turns: vec![
            Turn::ToolCall {
                id: "write-model-recovery-tracked".to_string(),
                name: "write".to_string(),
                args: serde_json::json!({
                    "path": "service/README.md",
                    "content": TRACKED_CONTENT,
                }),
            },
            Turn::ToolCall {
                id: "write-model-recovery-untracked".to_string(),
                name: "write".to_string(),
                args: serde_json::json!({
                    "path": format!("service/{UNTRACKED_PATH}"),
                    "content": UNTRACKED_CONTENT,
                }),
            },
        ],
        usage: Default::default(),
        stop: StopReason::ToolCalls,
    }
}

fn read_predecessor_work() -> Reply {
    Reply {
        turns: vec![
            Turn::ToolCall {
                id: "read-model-recovery-tracked".to_string(),
                name: "read".to_string(),
                args: serde_json::json!({ "path": "service/README.md" }),
            },
            Turn::ToolCall {
                id: "read-model-recovery-untracked".to_string(),
                name: "read".to_string(),
                args: serde_json::json!({ "path": format!("service/{UNTRACKED_PATH}") }),
            },
        ],
        usage: Default::default(),
        stop: StopReason::ToolCalls,
    }
}

fn non_retryable_model_failure() -> ScriptAction {
    ScriptAction::HttpError(HttpError::provider(
        401,
        PROVIDER_CODE,
        "Fixture authentication failed.",
    ))
}

pub(super) fn assert_rotation_result(result: &JobResult) -> (String, String) {
    assert_eq!(result.status, ResultStatus::Failure);
    assert!(result.repos.is_empty());
    let failure = result.failure.as_ref().expect("rotation failure details");
    assert_eq!(failure.class, FailureClass::Transient);
    let diagnostic = failure
        .model_failure
        .as_ref()
        .unwrap_or_else(|| panic!("rotation omitted typed model diagnostic: {failure:?}"));
    assert_eq!(diagnostic.category, ModelFailureCategoryV1::Authentication);
    assert!(!diagnostic.retryable);
    assert_eq!(diagnostic.http_status, Some(401));
    assert_eq!(
        diagnostic.provider_error_code.as_deref(),
        Some(PROVIDER_CODE)
    );
    let recovery = failure
        .session_recovery
        .as_ref()
        .expect("rotation has durable recovery evidence");
    assert_eq!(recovery.action, SessionRecoveryActionV1::RotateSession);
    assert_eq!(recovery.failure_epoch, 1);
    assert_eq!(recovery.failure_count, 1);
    assert_eq!(recovery.attempt_id, result.attempt_id.as_deref().unwrap());
    assert!(recovery.prior_session_id.is_none());
    let fresh = recovery
        .new_session_id
        .clone()
        .expect("rotation names the fresh session");
    assert_ne!(recovery.current_session_id, fresh);
    assert_eq!(
        recovery.evidence_location,
        ".temper-agent-session/state.json"
    );
    (recovery.current_session_id.clone(), fresh)
}

pub(super) fn assert_park_result(
    result: &JobResult,
    prior_session_id: &str,
    fresh_session_id: &str,
) {
    assert_eq!(result.status, ResultStatus::Failure);
    assert!(result.repos.is_empty());
    let failure = result.failure.as_ref().expect("park failure details");
    assert_eq!(failure.class, FailureClass::Permanent);
    let diagnostic = failure
        .model_failure
        .as_ref()
        .expect("park has a typed model diagnostic");
    assert_eq!(diagnostic.category, ModelFailureCategoryV1::Authentication);
    assert!(!diagnostic.retryable);
    let recovery = failure
        .session_recovery
        .as_ref()
        .expect("park has durable recovery evidence");
    assert_eq!(recovery.action, SessionRecoveryActionV1::ParkForHuman);
    assert_eq!(recovery.failure_epoch, 1);
    assert_eq!(recovery.failure_count, 1);
    assert_eq!(recovery.current_session_id, fresh_session_id);
    assert_eq!(recovery.prior_session_id.as_deref(), Some(prior_session_id));
    assert!(recovery.new_session_id.is_none());
    assert_eq!(recovery.attempt_id, result.attempt_id.as_deref().unwrap());
}

pub(super) fn assert_rotated_ledger(
    ledger: &AgentSessionLedger,
    result: &JobResult,
    prior_session_id: &str,
    fresh_session_id: &str,
) {
    assert_eq!(ledger.active_session.session_id, fresh_session_id);
    let prior = ledger
        .prior_session
        .as_ref()
        .expect("consumed session is archived");
    assert_eq!(prior.session.session_id, prior_session_id);
    assert_eq!(
        prior.failed_attempt_id,
        result.attempt_id.as_deref().unwrap()
    );
    assert_eq!(prior.consecutive_terminal_count, 1);
    assert_eq!(
        prior.model_failure.category,
        ModelFailureCategoryV1::Authentication
    );
    assert_eq!(ledger.failure_epoch, 1);
    assert_eq!(ledger.consecutive_terminal_count, 0);
    assert!(ledger.rotation_consumed);
    assert_eq!(
        ledger.accounted_attempt_id.as_deref(),
        result.attempt_id.as_deref()
    );
    assert_eq!(
        ledger
            .recovery_decision
            .as_ref()
            .expect("rotation decision remains durable")
            .action,
        SessionRecoveryActionV1::RotateSession
    );
}

pub(super) fn assert_dirty_recovery_work(checkout: &Path) {
    assert_eq!(
        std::fs::read_to_string(checkout.join("README.md")).expect("tracked recovery work"),
        TRACKED_CONTENT
    );
    assert_eq!(
        std::fs::read_to_string(checkout.join(UNTRACKED_PATH)).expect("untracked recovery work"),
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
        lines.contains(&"?? MODEL-RECOVERY.txt"),
        "untracked edit missing: {status}"
    );
}

pub(super) fn observed_session_ids(stack: &HermeticRealStack) -> Vec<String> {
    stack
        .observed_agent_sessions()
        .into_iter()
        .map(|session| session.expect("engineer run has session state").session_id)
        .collect()
}

pub(super) fn sorted_branches(stack: &HermeticRealStack) -> Vec<String> {
    let mut branches = stack
        .origin_branches(stack.primary_repo_path())
        .expect("origin branch inventory");
    branches.sort();
    branches
}

pub(super) async fn await_result_after_attempt(
    cx: &skein::cx::Cx,
    stack: &mut HermeticRealStack,
    ignored_attempt: &str,
    timeout: Duration,
) -> Result<JobResult, String> {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(format!(
                "timed out after {timeout:?} waiting for a result after attempt {ignored_attempt}"
            ));
        }
        let result = stack.await_worker_result(cx, remaining).await?;
        if result.attempt_id.as_deref() != Some(ignored_attempt) {
            return Ok(result);
        }
    }
}

pub(super) async fn wait_for_accepted_release_count(
    stack: &HermeticRealStack,
    cx: &skein::cx::Cx,
    attempt_id: &str,
) -> usize {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let count = stack
            .published_releases()
            .iter()
            .filter(|release| {
                release.attempt_id.as_deref() == Some(attempt_id)
                    && release.disposition == ReleaseDisposition::Accepted
            })
            .count();
        if count > 0 {
            return count;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for the accepted release for {attempt_id}"
        );
        temper_engine_io::runtime::sleep_for(cx, Duration::from_millis(10)).await;
    }
}

pub(super) async fn wait_for_ready_issue(stack: &HermeticRealStack, cx: &skein::cx::Cx) -> Issue {
    wait_for_issue(stack, cx, "released ready issue", |issue| {
        issue.labels.iter().any(|label| label == "ready")
            && !issue.labels.iter().any(|label| label == "in-progress")
            && issue.assignees.is_empty()
    })
    .await
}

pub(super) async fn wait_for_parked_issue(stack: &HermeticRealStack, cx: &skein::cx::Cx) -> Issue {
    let issue = wait_for_issue(stack, cx, "human-attention model park", |issue| {
        issue.labels.iter().any(|label| label == "needs-human")
            && !issue.labels.iter().any(|label| label == "ready")
            && !issue.labels.iter().any(|label| label == "in-progress")
            && issue.assignees.is_empty()
    })
    .await;
    assert_parked_without_assignment(&issue);
    issue
}

async fn wait_for_issue(
    stack: &HermeticRealStack,
    cx: &skein::cx::Cx,
    description: &str,
    predicate: impl Fn(&Issue) -> bool,
) -> Issue {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let issue = current_issue(stack).await;
        if predicate(&issue) {
            return issue;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {description}; labels={:?} assignees={:?}",
            issue.labels,
            issue.assignees
        );
        temper_engine_io::runtime::sleep_for(cx, Duration::from_millis(10)).await;
    }
}

pub(super) async fn current_issue(stack: &HermeticRealStack) -> Issue {
    stack
        .forge()
        .get_issue_by_number(stack.primary_repo_id(), stack.issue_number())
        .await
        .expect("issue lookup succeeds")
        .expect("source issue exists")
}

pub(super) async fn comments(
    stack: &HermeticRealStack,
    issue: &Issue,
) -> Vec<temper_forge_model::Comment> {
    stack
        .forge()
        .list_issue_comments(&issue.id)
        .await
        .expect("issue comments list")
}

pub(super) async fn assert_no_human_attention(stack: &HermeticRealStack) {
    let issue = current_issue(stack).await;
    assert!(!issue.labels.iter().any(|label| label == "needs-human"));
    assert!(comments(stack, &issue).await.is_empty());
}

pub(super) fn assert_actionable_park_audit(audit: &str) {
    for expected in [
        "bounded model recovery was exhausted",
        "failure_epoch: `1`",
        "failure_count: `1`",
        "action: `park_for_human`",
        "category: `authentication`",
        "retryable: `false`",
        "http_status: `401`",
        PROVIDER_CODE,
        ".temper-agent-session/state.json",
        "Operator action:",
        "temper:comment-key=model_recovery_park:",
    ] {
        assert!(
            audit.contains(expected),
            "audit omitted {expected}: {audit}"
        );
    }
}

pub(super) fn assert_parked_without_assignment(issue: &Issue) {
    assert!(issue.labels.iter().any(|label| label == "needs-human"));
    assert!(!issue.labels.iter().any(|label| label == "ready"));
    assert!(!issue.labels.iter().any(|label| label == "in-progress"));
    assert!(issue.assignees.is_empty());
    let metadata = parse_metadata_block(&issue.body)
        .expect("parked issue metadata parses")
        .unwrap_or_default();
    assert!(metadata.assignment.is_none());
    assert!(metadata.lease.is_none());
}

pub(super) async fn redeliver_result(stack: &HermeticRealStack, result: JobResult) {
    let response = stack
        .daemon()
        .deliver_protocol_message(WorkerProtocolMessage::Result(result))
        .await
        .expect("duplicate terminal delivery receives a protocol response")
        .expect("duplicate terminal delivery is acknowledged");
    assert!(
        matches!(response, WorkerProtocolMessage::Release(_)),
        "duplicate terminal delivery should receive a release, got {response:?}"
    );
}
