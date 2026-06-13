//! End-to-end tests for [`CodingExecutor`]: workspace prep, the agent turn (an
//! in-process [`FakeAgentRunner`] standing in for the out-of-process agent), and
//! the result → [`JobOutcome`] mapping (commit/push on the writable head path,
//! verdict routing otherwise).
//!
//! These exercise the executor's workspace/git/result-mapping logic, which is
//! independent of *how* the agent turn is produced. The fake is an in-process
//! [`AgentRunner`]: it captures the typed [`WorkspaceContext`] it receives,
//! optionally writes a product diff into the checkout, and returns a scripted
//! [`WorkspaceResult`] (or [`AgentRunError`]). The real out-of-process boundary
//! (spawning an agent program, streaming step-progress) is covered separately by
//! `coding_worker_e2e.rs`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::{Value, json};
use smith_agent_protocol::{WorkspaceContext, WorkspaceResultChild};
use smith_worker::{
    AgentRunError, AgentRunner, CodingExecutor, CodingExecutorConfig, JobExecutor, JobOutcome,
    ProgressSink, RoleGitIdentity, WorkspaceResult,
};
use temper_worker_protocol::{Artifact, Assign, FailureClass, JobChild, WORKER_PROTOCOL_VERSION};
use tempfile::TempDir;

#[tokio::test]
async fn success_path_commits_pushes_and_reports_branch() {
    let fixture = Fixture::new();
    let agent = AgentBehavior::Success.runner();
    let executor = fixture.executor(agent.clone(), true);

    let outcome = executor
        .execute(assign("agent/pr-for-code-7", "pr-for-code-7"))
        .await;

    let (branch_name, head_sha, summary) = expect_success(outcome);
    assert_eq!(branch_name, "agent/pr-for-code-7");
    assert_is_sha(&head_sha);
    assert_eq!(summary.as_deref(), Some("did the work"));

    assert_eq!(
        git_output([
            "-C",
            path_str(&fixture.origin),
            "rev-parse",
            "refs/heads/agent/pr-for-code-7",
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
            "refs/heads/agent/pr-for-code-7",
        ]),
        "Implement pr-for-code-7"
    );
    assert_eq!(
        git_output([
            "-C",
            path_str(&fixture.origin),
            "log",
            "-1",
            "--format=%b",
            "refs/heads/agent/pr-for-code-7",
        ]),
        "Closes #7"
    );
    assert_eq!(
        git_output([
            "-C",
            path_str(&fixture.origin),
            "log",
            "-1",
            "--format=%an <%ae>|%cn <%ce>",
            "refs/heads/agent/pr-for-code-7",
        ]),
        "Smith Engineer <smith-engineer@example.test>|Smith Engineer <smith-engineer@example.test>"
    );
    assert_eq!(
        git_output([
            "-C",
            path_str(&fixture.origin),
            "show",
            "refs/heads/agent/pr-for-code-7:agent-output.txt",
        ]),
        "agent diff"
    );
}

#[tokio::test]
async fn context_shape_matches_temper_coding_agent_contract() {
    let fixture = Fixture::new();
    let agent = AgentBehavior::Success.runner();
    let executor = fixture.executor(agent.clone(), true);

    expect_success(
        executor
            .execute(assign("agent/pr-for-code-7", "pr-for-code-7"))
            .await,
    );

    let context = agent.captured_context();
    assert_workspace_context(
        &context,
        ExpectedWorkspaceContext {
            role: "engineer",
            queue: "code_ready",
            kind: "code",
            checkout: "writable",
            allowed_verdicts: &[],
            branch_hint: "agent/pr-for-code-7",
            correlation_key: "pr-for-code-7",
            target: "Issue { number: ItemNumber(7) }",
            artifact_type: "issue",
        },
    );
}

#[tokio::test]
async fn context_shape_passes_through_read_only_capability_and_verdicts() {
    let fixture = Fixture::new();
    let agent = AgentBehavior::ReadOnlyVerdict.runner();
    let executor = fixture.executor(agent.clone(), true);

    expect_verdict(
        executor
            .execute(assign_with_context(
                "triage-7",
                read_only_job_context("agent/triage-7", "triage-7"),
            ))
            .await,
    );

    let context = agent.captured_context();
    assert_workspace_context(
        &context,
        ExpectedWorkspaceContext {
            role: "architect",
            queue: "design_review",
            kind: "triage",
            checkout: "read_only",
            allowed_verdicts: &["ready_code", "needs_design"],
            branch_hint: "agent/triage-7",
            correlation_key: "triage-7",
            target: "Issue { number: ItemNumber(7) }",
            artifact_type: "issue",
        },
    );
}

#[tokio::test]
async fn review_context_shape_carries_pull_request_target() {
    let fixture = Fixture::new();
    let agent = AgentBehavior::ReviewApprove.runner();
    let executor = fixture.executor(agent.clone(), true);

    expect_verdict(
        executor
            .execute(pr_assign("agent/review-7", "review-7", pr_job_context))
            .await,
    );

    let context = agent.captured_context();
    assert_workspace_context(
        &context,
        ExpectedWorkspaceContext {
            role: "reviewer",
            queue: "pr_needs_review",
            kind: "implementation_pr",
            checkout: "pull_request_read_only",
            allowed_verdicts: &["approve", "changes", "escalate"],
            branch_hint: "agent/review-7",
            correlation_key: "review-7",
            target: "PullRequest { number: ItemNumber(7) }",
            artifact_type: "pull_request",
        },
    );
}

struct ExpectedWorkspaceContext<'a> {
    role: &'a str,
    queue: &'a str,
    kind: &'a str,
    checkout: &'a str,
    allowed_verdicts: &'a [&'a str],
    branch_hint: &'a str,
    correlation_key: &'a str,
    target: &'a str,
    artifact_type: &'a str,
}

fn assert_workspace_context(context: &WorkspaceContext, expected: ExpectedWorkspaceContext<'_>) {
    assert_eq!(context.repository.id, "acme/service");
    assert_eq!(context.repository.owner, "acme");
    assert_eq!(context.repository.name, "service");
    assert_eq!(context.repository.default_branch, "main");
    assert_eq!(context.work_item.role, expected.role);
    assert_eq!(context.work_item.queue, expected.queue);
    assert_eq!(context.work_item.kind, expected.kind);
    assert_eq!(context.work_item.target, expected.target);
    assert_eq!(context.base_branch, "main");
    assert_eq!(context.branch_hint, expected.branch_hint);
    assert_eq!(context.correlation_key, expected.correlation_key);
    assert_eq!(context.checkout.as_deref(), Some(expected.checkout));
    assert_eq!(
        context.allowed_verdicts,
        expected
            .allowed_verdicts
            .iter()
            .map(|verdict| (*verdict).to_string())
            .collect::<Vec<_>>()
    );
    assert_eq!(context.guidance.role_guidance, None);
    assert_eq!(context.guidance.tool_guidance, None);
    assert!(context.guidance.tool_constraints.is_empty());

    let inner: Value =
        serde_json::from_str(&context.work_item.context).expect("inner work item JSON parses");
    assert_eq!(inner["repository"], "acme/service");
    assert_eq!(inner["role"], expected.role);
    assert_eq!(inner["queue"], expected.queue);
    assert_eq!(inner["kind"], expected.kind);
    assert_eq!(inner["artifact"]["type"], expected.artifact_type);
    assert_eq!(inner["artifact"]["number"], 7);
    assert_eq!(inner["artifact"]["title"], "Implement the thing");
    assert_eq!(inner["artifact"]["body"], "Detailed issue body");
    assert_eq!(inner["artifact"]["labels"], json!(["code", "ready"]));
    assert_eq!(inner["artifact"]["state"], "Open");
}

#[tokio::test]
async fn workspace_is_reused_across_successful_jobs_for_same_repo_and_role() {
    let fixture = Fixture::new();
    let executor = fixture.executor(AgentBehavior::Success.runner(), true);

    expect_success(
        executor
            .execute(assign("agent/pr-for-code-7", "pr-for-code-7"))
            .await,
    );
    let workspace_path = fixture.workspace_root.join("acme__service/engineer");
    assert!(workspace_path.exists());
    let sentinel = workspace_path.join(".git/smith-sentinel");
    fs::write(&sentinel, "keep object cache").expect("write sentinel");

    let (branch_name, head_sha, _) = expect_success(
        executor
            .execute(assign("agent/pr-for-code-8", "pr-for-code-8"))
            .await,
    );

    assert_eq!(branch_name, "agent/pr-for-code-8");
    assert!(
        sentinel.exists(),
        "prepare must reuse the existing checkout"
    );
    assert_eq!(
        git_output([
            "-C",
            path_str(&fixture.origin),
            "rev-parse",
            "refs/heads/agent/pr-for-code-8",
        ]),
        head_sha
    );
}

#[tokio::test]
async fn malformed_payload_maps_to_protocol_failure() {
    let fixture = Fixture::new();
    let executor = fixture.executor(AgentBehavior::NoDiff.runner(), true);

    let outcome = executor
        .execute(Assign {
            job_payload: json!({"nope": true}),
            ..assign("agent/pr-for-code-7", "pr-for-code-7")
        })
        .await;

    expect_failure_class(outcome, FailureClass::Protocol);
}

#[tokio::test]
async fn missing_enriched_artifact_maps_to_protocol_failure() {
    let fixture = Fixture::new();
    let executor = fixture.executor(AgentBehavior::NoDiff.runner(), true);
    let mut context = job_context("agent/pr-for-code-7", "pr-for-code-7");
    context.artifact = None;

    let outcome = executor
        .execute(Assign {
            job_payload: serde_json::to_value(context).expect("JobContext serializes"),
            ..assign("agent/pr-for-code-7", "pr-for-code-7")
        })
        .await;

    let message = expect_failure_class(outcome, FailureClass::Protocol);
    assert!(
        message.contains("artifact"),
        "message should name missing field: {message}"
    );
}

#[tokio::test]
async fn missing_role_identity_maps_to_permanent_failure() {
    let fixture = Fixture::new();
    let executor = fixture.executor(AgentBehavior::NoDiff.runner(), false);

    let outcome = executor
        .execute(assign("agent/pr-for-code-7", "pr-for-code-7"))
        .await;

    let message = expect_failure_class(outcome, FailureClass::Permanent);
    assert!(
        message.contains("worker has no git identity for role engineer"),
        "unexpected message: {message}"
    );
}

#[tokio::test]
async fn transient_agent_error_maps_to_transient_failure() {
    let fixture = Fixture::new();
    let executor = fixture.executor(AgentBehavior::TransientError.runner(), true);

    let outcome = executor
        .execute(assign("agent/pr-for-code-7", "pr-for-code-7"))
        .await;

    let message = expect_failure_class(outcome, FailureClass::Transient);
    assert!(
        message.contains("provider transport reset"),
        "transient error message missing: {message}"
    );
}

#[tokio::test]
async fn zero_diff_maps_to_permanent_failure() {
    let fixture = Fixture::new();
    let executor = fixture.executor(AgentBehavior::NoDiff.runner(), true);

    let outcome = executor
        .execute(assign("agent/pr-for-code-7", "pr-for-code-7"))
        .await;

    let message = expect_failure_class(outcome, FailureClass::Permanent);
    assert!(
        message.contains("agent produced no diff"),
        "unexpected message: {message}"
    );
}

#[tokio::test]
async fn verdict_result_maps_to_permanent_failure() {
    let fixture = Fixture::new();
    let executor = fixture.executor(AgentBehavior::Verdict.runner(), true);

    let outcome = executor
        .execute(assign("agent/pr-for-code-7", "pr-for-code-7"))
        .await;

    let message = expect_failure_class(outcome, FailureClass::Permanent);
    assert!(
        message.contains("needs_design"),
        "message should name the unsupported verdict: {message}"
    );
}

#[tokio::test]
async fn read_only_job_returns_verdict_and_body() {
    let fixture = Fixture::new();
    let executor = fixture.executor(AgentBehavior::ReadOnlyVerdict.runner(), true);

    let (verdict, body, summary, children) = expect_verdict(
        executor
            .execute(assign_with_context(
                "triage-7",
                read_only_job_context("agent/triage-7", "triage-7"),
            ))
            .await,
    );

    assert_eq!(verdict, "ready_code");
    assert_eq!(body.as_deref(), Some("rewritten"));
    assert_eq!(summary.as_deref(), Some("did triage"));
    assert!(children.is_empty());
    assert_no_origin_branch(&fixture, "agent/triage-7");
}

#[tokio::test]
async fn read_only_job_with_diff_still_returns_verdict_without_push() {
    let fixture = Fixture::new();
    let executor = fixture.executor(AgentBehavior::ReadOnlyVerdictWithDiff.runner(), true);

    let (verdict, body, summary, children) = expect_verdict(
        executor
            .execute(assign_with_context(
                "triage-with-diff-7",
                read_only_job_context("agent/triage-with-diff-7", "triage-with-diff-7"),
            ))
            .await,
    );

    assert_eq!(verdict, "ready_code");
    assert_eq!(body.as_deref(), Some("rewritten"));
    assert_eq!(summary.as_deref(), Some("did triage"));
    assert!(children.is_empty());
    assert_no_origin_branch(&fixture, "agent/triage-with-diff-7");
    assert_workspace_clean(&fixture, "architect");
}

#[tokio::test]
async fn read_only_breakdown_verdict_passes_children_through() {
    let fixture = Fixture::new();
    let executor = fixture.executor(AgentBehavior::ReadOnlyBreakdownVerdict.runner(), true);
    let mut context = read_only_job_context("agent/breakdown-7", "breakdown-7");
    context.allowed_verdicts = vec!["needs_breakdown".to_string()];

    let (verdict, body, summary, children) = expect_verdict(
        executor
            .execute(assign_with_context("breakdown-7", context))
            .await,
    );

    assert_eq!(verdict, "needs_breakdown");
    assert_eq!(body, None);
    assert_eq!(summary.as_deref(), Some("planned breakdown"));
    assert_eq!(
        children,
        vec![
            JobChild {
                slug: "api-schema".to_string(),
                title: "Define the API schema".to_string(),
                body: "Write the shared API schema.".to_string(),
                labels: vec!["code".to_string(), "ready".to_string()],
                depends_on: Vec::new(),
                target_repo: None,
            },
            JobChild {
                slug: "web-client".to_string(),
                title: "Implement the web client".to_string(),
                body: "Build the web client against the API schema.".to_string(),
                labels: Vec::new(),
                depends_on: vec!["api-schema".to_string()],
                target_repo: Some("acme/other".to_string()),
            },
        ]
    );
    assert_no_origin_branch(&fixture, "agent/breakdown-7");
}

#[tokio::test]
async fn read_only_job_without_verdict_is_permanent() {
    let fixture = Fixture::new();
    let executor = fixture.executor(AgentBehavior::NoDiff.runner(), true);

    let outcome = executor
        .execute(assign_with_context(
            "triage-no-verdict-7",
            read_only_job_context("agent/triage-no-verdict-7", "triage-no-verdict-7"),
        ))
        .await;

    let message = expect_failure_class(outcome, FailureClass::Permanent);
    assert!(
        message.contains("read-only job returned no verdict"),
        "unexpected message: {message}"
    );
    assert_no_origin_branch(&fixture, "agent/triage-no-verdict-7");
}

#[tokio::test]
async fn read_only_job_with_undeclared_verdict_is_permanent() {
    let fixture = Fixture::new();
    let executor = fixture.executor(AgentBehavior::UndeclaredVerdict.runner(), true);

    let outcome = executor
        .execute(assign_with_context(
            "triage-undeclared-7",
            read_only_job_context("agent/triage-undeclared-7", "triage-undeclared-7"),
        ))
        .await;

    let message = expect_failure_class(outcome, FailureClass::Permanent);
    assert!(
        message.contains("needs_breakdown"),
        "message should name the emitted verdict: {message}"
    );
    assert!(
        message.contains("ready_code") && message.contains("needs_design"),
        "message should name the allowed vocabulary: {message}"
    );
    assert_no_origin_branch(&fixture, "agent/triage-undeclared-7");
}

#[tokio::test]
async fn review_job_returns_approve_verdict() {
    let fixture = Fixture::new();
    let agent = AgentBehavior::ReviewApprove.runner();
    let executor = fixture.executor(agent.clone(), true);

    let (verdict, body, summary, children) = expect_verdict(
        executor
            .execute(pr_assign("agent/review-7", "review-7", pr_job_context))
            .await,
    );

    assert_eq!(verdict, "approve");
    assert_eq!(body, None);
    assert_eq!(summary.as_deref(), Some("looks good"));
    assert!(children.is_empty());
    // The runner ran in the prepared PR-head checkout: it observed the PR head
    // sha, confirming the executor checked out `refs/pull/7/head`.
    assert_eq!(agent.observed_head_sha(), fixture.pull_request_head_sha);
    assert_no_origin_branch(&fixture, "agent/review-7");
    assert_no_extra_origin_head_branches(&fixture, &["main"]);
}

#[tokio::test]
async fn review_job_changes_verdict_passes_review_body_through() {
    let fixture = Fixture::new();
    let executor = fixture.executor(AgentBehavior::ReviewChanges.runner(), true);

    let (verdict, body, summary, children) = expect_verdict(
        executor
            .execute(pr_assign(
                "agent/review-changes-7",
                "review-changes-7",
                pr_job_context,
            ))
            .await,
    );

    assert_eq!(verdict, "changes");
    assert_eq!(body.as_deref(), Some("please add error handling"));
    assert_eq!(summary.as_deref(), Some("needs error handling"));
    assert!(children.is_empty());
    assert_no_origin_branch(&fixture, "agent/review-changes-7");
    assert_no_extra_origin_head_branches(&fixture, &["main"]);
    assert_workspace_clean(&fixture, "reviewer");
}

#[tokio::test]
async fn review_job_missing_verdict_is_permanent_failure() {
    let fixture = Fixture::new();
    let executor = fixture.executor(AgentBehavior::ReviewMissingVerdict.runner(), true);

    let outcome = executor
        .execute(pr_assign(
            "agent/review-missing-verdict-7",
            "review-missing-verdict-7",
            pr_job_context,
        ))
        .await;

    let message = expect_failure_class(outcome, FailureClass::Permanent);
    assert!(
        message.contains("read-only job returned no verdict"),
        "unexpected message: {message}"
    );
    assert_no_origin_branch(&fixture, "agent/review-missing-verdict-7");
    assert_no_extra_origin_head_branches(&fixture, &["main"]);
}

#[tokio::test]
async fn review_job_undeclared_verdict_is_permanent_failure() {
    let fixture = Fixture::new();
    let executor = fixture.executor(AgentBehavior::ReviewUndeclaredVerdict.runner(), true);

    let outcome = executor
        .execute(pr_assign(
            "agent/review-undeclared-7",
            "review-undeclared-7",
            pr_job_context,
        ))
        .await;

    let message = expect_failure_class(outcome, FailureClass::Permanent);
    assert!(
        message.contains("merge_now"),
        "message should name the emitted verdict: {message}"
    );
    assert!(
        message.contains("approve") && message.contains("changes") && message.contains("escalate"),
        "message should name the allowed vocabulary: {message}"
    );
    assert_no_origin_branch(&fixture, "agent/review-undeclared-7");
    assert_no_extra_origin_head_branches(&fixture, &["main"]);
}

#[tokio::test]
async fn writable_job_with_allowed_escalation_verdict_returns_verdict() {
    let fixture = Fixture::new();
    let executor = fixture.executor(AgentBehavior::WritableVerdict.runner(), true);

    let (verdict, body, summary, children) = expect_verdict(
        executor
            .execute(assign_with_context(
                "pr-for-code-7",
                writable_job_context_with_allowed_verdicts(
                    "agent/pr-for-code-7",
                    "pr-for-code-7",
                    &["needs_architect"],
                ),
            ))
            .await,
    );

    assert_eq!(verdict, "needs_architect");
    assert_eq!(body.as_deref(), Some("blocked"));
    assert_eq!(summary.as_deref(), Some("cannot proceed"));
    assert!(children.is_empty());
    assert_no_origin_branch(&fixture, "agent/pr-for-code-7");
    assert_workspace_clean(&fixture, "engineer");
}

// ---------------------------------------------------------------------------
// In-process fake agent runner.
// ---------------------------------------------------------------------------

/// What the fake agent does for one turn. Each variant mirrors a behavior the
/// old shell-script fakes produced, but expressed as in-process effects:
/// capture the context, optionally write a product diff into the checkout, and
/// return a [`WorkspaceResult`] or an [`AgentRunError`].
#[derive(Clone, Copy)]
enum AgentBehavior {
    /// Engineer head path: write a product diff, return a summary-only result.
    Success,
    /// A transient provider error (the in-process analog of the old non-zero
    /// subprocess exit).
    TransientError,
    /// Return a summary-only result and write no diff (engineer ⇒ "no diff").
    NoDiff,
    /// Engineer that checkpoint-commits and pushes its whole product itself
    /// (phase 6b), leaving a clean working tree for the executor.
    CheckpointCommits,
    /// Return a writable verdict the executor does not route (permanent).
    Verdict,
    /// Architect read-only `ready_code` verdict with a rewritten body.
    ReadOnlyVerdict,
    /// Architect `needs_breakdown` verdict with children.
    ReadOnlyBreakdownVerdict,
    /// Read-only verdict that also (wrongly) writes a diff; the executor must
    /// discard it and still route the verdict without pushing.
    ReadOnlyVerdictWithDiff,
    /// Read-only verdict outside the declared vocabulary (permanent).
    UndeclaredVerdict,
    /// Writable escalation verdict that also writes a diff to discard.
    WritableVerdict,
    /// Reviewer `approve` verdict (records the checked-out HEAD sha).
    ReviewApprove,
    /// Reviewer `changes` verdict with a review body; writes a diff to discard.
    ReviewChanges,
    /// Reviewer result with no verdict (permanent).
    ReviewMissingVerdict,
    /// Reviewer verdict outside the declared vocabulary (permanent).
    ReviewUndeclaredVerdict,
}

impl AgentBehavior {
    fn runner(self) -> FakeAgentRunner {
        FakeAgentRunner {
            behavior: self,
            captured: Arc::new(Mutex::new(None)),
            observed_head_sha: Arc::new(Mutex::new(None)),
        }
    }
}

#[derive(Clone)]
struct FakeAgentRunner {
    behavior: AgentBehavior,
    captured: Arc<Mutex<Option<WorkspaceContext>>>,
    observed_head_sha: Arc<Mutex<Option<String>>>,
}

impl FakeAgentRunner {
    /// The context the executor handed the runner (panics if the runner never
    /// ran).
    fn captured_context(&self) -> WorkspaceContext {
        self.captured
            .lock()
            .expect("capture lock")
            .clone()
            .expect("fake agent runner captured a context")
    }

    /// The `git HEAD` sha the runner saw in the prepared checkout.
    fn observed_head_sha(&self) -> String {
        self.observed_head_sha
            .lock()
            .expect("head lock")
            .clone()
            .expect("fake agent runner observed HEAD")
    }

    fn write_diff(cwd: &Path) {
        fs::write(cwd.join("agent-output.txt"), "agent diff\n").expect("write fake agent diff");
    }
}

impl AgentRunner for FakeAgentRunner {
    async fn run(
        &self,
        context: &WorkspaceContext,
        cwd: &Path,
        _progress: &dyn ProgressSink,
    ) -> Result<WorkspaceResult, AgentRunError> {
        *self.captured.lock().expect("capture lock") = Some(context.clone());
        *self.observed_head_sha.lock().expect("head lock") =
            Some(git_output(["-C", path_str(cwd), "rev-parse", "HEAD"]));

        match self.behavior {
            AgentBehavior::Success => {
                Self::write_diff(cwd);
                Ok(WorkspaceResult {
                    summary: Some("did the work".to_string()),
                    ..WorkspaceResult::default()
                })
            }
            AgentBehavior::TransientError => Err(AgentRunError::transient(
                "LLM run failed: provider transport reset",
            )),
            AgentBehavior::NoDiff => Ok(WorkspaceResult {
                summary: Some("nothing changed".to_string()),
                ..WorkspaceResult::default()
            }),
            AgentBehavior::CheckpointCommits => {
                Self::write_diff(cwd);
                let work_branch = context.branch_hint.clone();
                git_output(["-C", path_str(cwd), "add", "-A"]);
                git_output([
                    "-C",
                    path_str(cwd),
                    "-c",
                    "user.name=Agent Checkpoint",
                    "-c",
                    "user.email=agent@example.test",
                    "commit",
                    "-m",
                    "checkpoint(step 2): push checkpoint",
                ]);
                git_output([
                    "-C",
                    path_str(cwd),
                    "push",
                    "origin",
                    &format!("HEAD:refs/heads/{work_branch}"),
                ]);
                Ok(WorkspaceResult {
                    summary: Some("checkpointed the work".to_string()),
                    ..WorkspaceResult::default()
                })
            }
            AgentBehavior::Verdict => Ok(WorkspaceResult {
                verdict: Some("needs_design".to_string()),
                summary: Some("cannot proceed".to_string()),
                ..WorkspaceResult::default()
            }),
            AgentBehavior::ReadOnlyVerdict => Ok(WorkspaceResult {
                verdict: Some("ready_code".to_string()),
                body: Some("rewritten".to_string()),
                summary: Some("did triage".to_string()),
                ..WorkspaceResult::default()
            }),
            AgentBehavior::ReadOnlyVerdictWithDiff => {
                Self::write_diff(cwd);
                Ok(WorkspaceResult {
                    verdict: Some("ready_code".to_string()),
                    body: Some("rewritten".to_string()),
                    summary: Some("did triage".to_string()),
                    ..WorkspaceResult::default()
                })
            }
            AgentBehavior::ReadOnlyBreakdownVerdict => Ok(WorkspaceResult {
                verdict: Some("needs_breakdown".to_string()),
                summary: Some("planned breakdown".to_string()),
                children: vec![
                    WorkspaceResultChild {
                        slug: "api-schema".to_string(),
                        title: "Define the API schema".to_string(),
                        body: "Write the shared API schema.".to_string(),
                        labels: vec!["code".to_string(), "ready".to_string()],
                        depends_on: Vec::new(),
                        target_repo: None,
                    },
                    WorkspaceResultChild {
                        slug: "web-client".to_string(),
                        title: "Implement the web client".to_string(),
                        body: "Build the web client against the API schema.".to_string(),
                        labels: Vec::new(),
                        depends_on: vec!["api-schema".to_string()],
                        target_repo: Some("acme/other".to_string()),
                    },
                ],
                ..WorkspaceResult::default()
            }),
            AgentBehavior::UndeclaredVerdict => Ok(WorkspaceResult {
                verdict: Some("needs_breakdown".to_string()),
                summary: Some("needs splitting".to_string()),
                ..WorkspaceResult::default()
            }),
            AgentBehavior::WritableVerdict => {
                Self::write_diff(cwd);
                Ok(WorkspaceResult {
                    verdict: Some("needs_architect".to_string()),
                    body: Some("blocked".to_string()),
                    summary: Some("cannot proceed".to_string()),
                    ..WorkspaceResult::default()
                })
            }
            AgentBehavior::ReviewApprove => Ok(WorkspaceResult {
                verdict: Some("approve".to_string()),
                summary: Some("looks good".to_string()),
                ..WorkspaceResult::default()
            }),
            AgentBehavior::ReviewChanges => {
                Self::write_diff(cwd);
                Ok(WorkspaceResult {
                    verdict: Some("changes".to_string()),
                    review_body: Some("please add error handling".to_string()),
                    summary: Some("needs error handling".to_string()),
                    ..WorkspaceResult::default()
                })
            }
            AgentBehavior::ReviewMissingVerdict => Ok(WorkspaceResult {
                summary: Some("no opinion".to_string()),
                ..WorkspaceResult::default()
            }),
            AgentBehavior::ReviewUndeclaredVerdict => Ok(WorkspaceResult {
                verdict: Some("merge_now".to_string()),
                ..WorkspaceResult::default()
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// Fixture + assertions (unchanged git/workspace machinery).
// ---------------------------------------------------------------------------

struct Fixture {
    temp: TempDir,
    origin: PathBuf,
    workspace_root: PathBuf,
    pull_request_head_sha: String,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("create temp dir");
        let git_root = temp.path().join("git");
        fs::create_dir_all(git_root.join("acme")).expect("create git root");
        let origin = git_root.join("acme/service.git");
        git(["init", "--bare", path_str(&origin)]);
        let pull_request_head_sha = seed_origin(&origin, temp.path());

        Self {
            workspace_root: temp.path().join("workspaces"),
            temp,
            origin,
            pull_request_head_sha,
        }
    }

    fn executor(
        &self,
        runner: FakeAgentRunner,
        include_identity: bool,
    ) -> CodingExecutor<FakeAgentRunner> {
        let mut role_identities = BTreeMap::new();
        if include_identity {
            for (role, user, email) in [
                ("engineer", "Smith Engineer", "smith-engineer@example.test"),
                (
                    "architect",
                    "Smith Architect",
                    "smith-architect@example.test",
                ),
                ("reviewer", "Smith Reviewer", "smith-reviewer@example.test"),
            ] {
                role_identities.insert(
                    role.to_string(),
                    RoleGitIdentity {
                        user: user.to_string(),
                        email: email.to_string(),
                        token: "test-token".to_string(),
                    },
                );
            }
        }

        CodingExecutor::new(
            CodingExecutorConfig {
                workspace_root: self.workspace_root.clone(),
                git_base_url: format!("file://{}/git", path_str(self.temp.path())),
                role_identities,
            },
            Arc::new(runner),
        )
    }
}

fn assign(branch_hint: &str, correlation_key: &str) -> Assign {
    Assign {
        protocol_version: WORKER_PROTOCOL_VERSION,
        job_id: format!("acme/service/issue-7/engineer/{correlation_key}"),
        role: "engineer".to_string(),
        repo: "acme/service".to_string(),
        artifact: Artifact {
            item: json!(7),
            kind: "issue".to_string(),
        },
        job_payload: serde_json::to_value(job_context(branch_hint, correlation_key))
            .expect("JobContext serializes"),
    }
}

fn pr_assign(
    branch_hint: &str,
    correlation_key: &str,
    context_builder: fn(&str, &str) -> TestJobContext,
) -> Assign {
    let context = context_builder(branch_hint, correlation_key);
    Assign {
        protocol_version: WORKER_PROTOCOL_VERSION,
        job_id: format!("acme/service/pull-7/reviewer/{correlation_key}"),
        role: "reviewer".to_string(),
        repo: "acme/service".to_string(),
        artifact: Artifact {
            item: json!(7),
            kind: "pull_request".to_string(),
        },
        job_payload: serde_json::to_value(context).expect("PR JobContext serializes"),
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct TestJobContext {
    role: String,
    repo: String,
    queue: String,
    artifact_kind: String,
    repository: Option<TestJobRepository>,
    base_branch: Option<String>,
    branch_hint: Option<String>,
    correlation_key: Option<String>,
    artifact: Option<TestJobArtifactSnapshot>,
    action: Option<String>,
    checkout_capability: Option<String>,
    allowed_verdicts: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct TestJobRepository {
    owner: String,
    name: String,
    default_branch: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct TestJobArtifactSnapshot {
    number: u64,
    title: String,
    body: String,
    labels: Vec<String>,
    state: String,
}

fn job_context(branch_hint: &str, correlation_key: &str) -> TestJobContext {
    TestJobContext {
        role: "engineer".to_string(),
        repo: "acme/service".to_string(),
        queue: "code_ready".to_string(),
        artifact_kind: "code".to_string(),
        repository: Some(TestJobRepository {
            owner: "acme".to_string(),
            name: "service".to_string(),
            default_branch: "main".to_string(),
        }),
        base_branch: Some("main".to_string()),
        branch_hint: Some(branch_hint.to_string()),
        correlation_key: Some(correlation_key.to_string()),
        artifact: Some(TestJobArtifactSnapshot {
            number: 7,
            title: "Implement the thing".to_string(),
            body: "Detailed issue body".to_string(),
            labels: vec!["code".to_string(), "ready".to_string()],
            state: "Open".to_string(),
        }),
        action: None,
        checkout_capability: None,
        allowed_verdicts: vec![],
    }
}

fn read_only_job_context(branch_hint: &str, correlation_key: &str) -> TestJobContext {
    let mut context = job_context(branch_hint, correlation_key);
    context.role = "architect".to_string();
    context.queue = "design_review".to_string();
    context.artifact_kind = "triage".to_string();
    context.action = Some("triage_to_code".to_string());
    context.checkout_capability = Some("read_only".to_string());
    context.allowed_verdicts = vec!["ready_code".to_string(), "needs_design".to_string()];
    context
}

fn writable_job_context_with_allowed_verdicts(
    branch_hint: &str,
    correlation_key: &str,
    allowed_verdicts: &[&str],
) -> TestJobContext {
    let mut context = job_context(branch_hint, correlation_key);
    context.action = Some("open_pr".to_string());
    context.checkout_capability = Some("writable".to_string());
    context.allowed_verdicts = allowed_verdicts
        .iter()
        .map(|verdict| (*verdict).to_string())
        .collect();
    context
}

fn pr_job_context(branch_hint: &str, correlation_key: &str) -> TestJobContext {
    let mut context = job_context(branch_hint, correlation_key);
    context.role = "reviewer".to_string();
    context.queue = "pr_needs_review".to_string();
    context.artifact_kind = "implementation_pr".to_string();
    context.action = Some("review_pr".to_string());
    context.checkout_capability = Some("pull_request_read_only".to_string());
    context.allowed_verdicts = vec![
        "approve".to_string(),
        "changes".to_string(),
        "escalate".to_string(),
    ];
    context
}

fn assign_with_context(correlation_key: &str, context: TestJobContext) -> Assign {
    let role = context.role.clone();
    Assign {
        protocol_version: WORKER_PROTOCOL_VERSION,
        job_id: format!("acme/service/issue-7/{role}/{correlation_key}"),
        role,
        repo: "acme/service".to_string(),
        artifact: Artifact {
            item: json!(7),
            kind: "issue".to_string(),
        },
        job_payload: serde_json::to_value(context).expect("JobContext serializes"),
    }
}

fn seed_origin(origin: &Path, temp: &Path) -> String {
    let seed = temp.join("seed");
    git(["init", "-b", "main", path_str(&seed)]);
    fs::write(seed.join("README.md"), "# seed\n").expect("write seed file");
    git([
        "-C",
        path_str(&seed),
        "-c",
        "user.name=Seed User",
        "-c",
        "user.email=seed@example.test",
        "add",
        "README.md",
    ]);
    git([
        "-C",
        path_str(&seed),
        "-c",
        "user.name=Seed User",
        "-c",
        "user.email=seed@example.test",
        "commit",
        "-m",
        "initial commit",
    ]);
    git([
        "-C",
        path_str(&seed),
        "remote",
        "add",
        "origin",
        path_str(origin),
    ]);
    git(["-C", path_str(&seed), "push", "origin", "main"]);

    git(["-C", path_str(&seed), "checkout", "-b", "review-head"]);
    fs::write(seed.join("pr-change.txt"), "pull request change\n").expect("write PR file");
    git([
        "-C",
        path_str(&seed),
        "-c",
        "user.name=Seed User",
        "-c",
        "user.email=seed@example.test",
        "add",
        "pr-change.txt",
    ]);
    git([
        "-C",
        path_str(&seed),
        "-c",
        "user.name=Seed User",
        "-c",
        "user.email=seed@example.test",
        "commit",
        "-m",
        "pull request change",
    ]);
    let pull_request_head_sha = git_output(["-C", path_str(&seed), "rev-parse", "HEAD"]);
    git([
        "-C",
        path_str(&seed),
        "push",
        "origin",
        "HEAD:refs/temper/seed/pr-7",
    ]);
    git([
        "-C",
        path_str(origin),
        "update-ref",
        "refs/pull/7/head",
        pull_request_head_sha.as_str(),
    ]);
    git([
        "-C",
        path_str(origin),
        "update-ref",
        "-d",
        "refs/temper/seed/pr-7",
    ]);
    pull_request_head_sha
}

fn expect_success(outcome: JobOutcome) -> (String, String, Option<String>) {
    match outcome {
        JobOutcome::Success { branch, summary } => (branch.name, branch.head_sha, summary),
        JobOutcome::Verdict {
            verdict,
            body,
            summary,
            children,
        } => {
            panic!("expected success, got verdict {verdict:?} {body:?} {summary:?} {children:?}")
        }
        JobOutcome::Failure { class, message } => {
            panic!("expected success, got {class:?}: {message}")
        }
    }
}

fn expect_verdict(outcome: JobOutcome) -> (String, Option<String>, Option<String>, Vec<JobChild>) {
    match outcome {
        JobOutcome::Verdict {
            verdict,
            body,
            summary,
            children,
        } => (verdict, body, summary, children),
        JobOutcome::Success { branch, summary } => {
            panic!("expected verdict, got success {branch:?} {summary:?}")
        }
        JobOutcome::Failure { class, message } => {
            panic!("expected verdict, got {class:?}: {message}")
        }
    }
}

fn expect_failure_class(outcome: JobOutcome, expected: FailureClass) -> String {
    match outcome {
        JobOutcome::Failure { class, message } => {
            assert_eq!(class, expected, "unexpected failure message: {message}");
            message
        }
        JobOutcome::Success { branch, summary } => {
            panic!("expected {expected:?} failure, got success {branch:?} {summary:?}")
        }
        JobOutcome::Verdict {
            verdict,
            body,
            summary,
            children,
        } => {
            panic!(
                "expected {expected:?} failure, got verdict {verdict:?} {body:?} {summary:?} {children:?}"
            )
        }
    }
}

fn assert_no_origin_branch(fixture: &Fixture, branch_name: &str) {
    let output = Command::new("git")
        .args([
            "-C",
            path_str(&fixture.origin),
            "show-ref",
            "--verify",
            &format!("refs/heads/{branch_name}"),
        ])
        .output()
        .expect("run git show-ref");
    assert!(
        !output.status.success(),
        "origin unexpectedly has branch {branch_name}: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

fn assert_no_extra_origin_head_branches(fixture: &Fixture, expected: &[&str]) {
    let output = git_output([
        "-C",
        path_str(&fixture.origin),
        "for-each-ref",
        "--format=%(refname:short)",
        "refs/heads",
    ]);
    let mut branches = if output.is_empty() {
        Vec::new()
    } else {
        output.lines().map(str::to_string).collect::<Vec<_>>()
    };
    branches.sort();

    let mut expected = expected
        .iter()
        .map(|branch| (*branch).to_string())
        .collect::<Vec<_>>();
    expected.sort();

    assert_eq!(branches, expected);
}

fn assert_workspace_clean(fixture: &Fixture, role: &str) {
    assert_eq!(
        git_output([
            "-C",
            path_str(&fixture.workspace_root.join("acme__service").join(role)),
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
        ]),
        ""
    );
}

fn git<const N: usize>(args: [&str; N]) {
    let output = Command::new("git")
        .args(args)
        .output()
        .expect("run git command");
    assert!(
        output.status.success(),
        "git command failed with status {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_output<const N: usize>(args: [&str; N]) -> String {
    let output = Command::new("git")
        .args(args)
        .output()
        .expect("run git command");
    assert!(
        output.status.success(),
        "git command failed with status {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    String::from_utf8(output.stdout)
        .expect("git stdout is utf-8")
        .trim_end_matches('\n')
        .to_string()
}

fn path_str(path: &Path) -> &str {
    path.as_os_str()
        .to_str()
        .unwrap_or_else(|| panic!("non-utf8 path: {:?}", path.as_os_str()))
}

fn assert_is_sha(value: &str) {
    assert_eq!(value.len(), 40, "not a full SHA: {value}");
    assert!(
        value.chars().all(|ch| ch.is_ascii_hexdigit()),
        "not hex: {value}"
    );
}

/// Phase 6b: an agent that checkpoint-commits and pushes its whole product
/// leaves a clean tree; the executor must still report success with the
/// checkpointed head rather than failing with "agent produced no diff".
#[tokio::test]
async fn checkpoint_committed_work_with_clean_tree_succeeds() {
    let fixture = Fixture::new();
    let executor = fixture.executor(AgentBehavior::CheckpointCommits.runner(), true);

    let outcome = executor
        .execute(assign("agent/pr-for-code-11", "pr-for-code-11"))
        .await;

    let (branch_name, head_sha, summary) = expect_success(outcome);
    assert_eq!(branch_name, "agent/pr-for-code-11");
    assert_eq!(summary.as_deref(), Some("checkpointed the work"));
    assert_eq!(
        git_output([
            "-C",
            path_str(&fixture.origin),
            "rev-parse",
            "refs/heads/agent/pr-for-code-11",
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
            "refs/heads/agent/pr-for-code-11",
        ]),
        "checkpoint(step 2): push checkpoint"
    );
}

/// Phase 6b: a re-dispatch for the same branch resumes from the pushed remote
/// branch (the prior dispatch's checkpoints) instead of resetting to base.
#[tokio::test]
async fn redispatch_resumes_from_pushed_work_branch() {
    let fixture = Fixture::new();
    let first = AgentBehavior::CheckpointCommits.runner();
    let executor = fixture.executor(first, true);
    let (_, first_head, _) = expect_success(
        executor
            .execute(assign("agent/pr-for-code-12", "pr-for-code-12"))
            .await,
    );

    // Second dispatch, same branch: the runner must observe the prior
    // checkpoint as HEAD, not a fresh base checkout.
    let second = AgentBehavior::Success.runner();
    let executor = fixture.executor(second.clone(), true);
    expect_success(
        executor
            .execute(assign("agent/pr-for-code-12", "pr-for-code-12"))
            .await,
    );
    assert_eq!(
        second.observed_head_sha(),
        first_head,
        "prepare must resume from the pushed work branch"
    );
}
