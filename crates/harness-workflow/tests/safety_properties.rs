//! Workflow safety-property tests (Phase 8).
//!
//! Each test asserts one of the safety properties listed in
//! `docs/reference/robustness-guarantees.md` over the checked-in five-role
//! fixture (and one inline three-gate workflow for the CI gate). All time is
//! supplied through fixed timestamps and all faults fire on fixed call counts,
//! so the suite is deterministic.

mod support;

use chrono::Duration;
use harness_forge::{
    BranchRef, CreateIssue, CreatePullRequest, Forge, IssueQuery, PullRequestQuery,
    PullRequestState, RepositoryId, UserId,
};
use harness_workflow::{
    parse_metadata_block, ArtifactSource, DefaultRecoveryPolicy, ExecutionError, Executor,
    InMemoryJournal, LeaseConflict, LeaseError, LeaseManager, LeasePolicy, RawWorkflowSpec,
    ReconcileFinding, RecoveryAction, RoleId, TransitionId,
};
use support::crash::{CrashForge, Fault, ForgeOp};
use support::{
    block_on, create_issue, create_pr, issue_body, new_repo, pr_labels, pr_state, ts, workflow,
    TestRoot,
};

/// An inline workflow whose merge gate requires CI, review, and testing together.
const THREE_GATE: &str = r#"{
    "name": "three-gate-merge",
    "roles": [
        {"id": "ci", "queues": []},
        {"id": "reviewer", "queues": []},
        {"id": "tester", "queues": []},
        {"id": "owner", "queues": []}
    ],
    "labels": [
        {"id": "implementation"},
        {"id": "ci-passed"},
        {"id": "review-approved"},
        {"id": "testing-passed"},
        {"id": "merge-ready"}
    ],
    "artifact_kinds": [
        {"id": "implementation_pr", "target": "pull_request", "identifying_labels": ["implementation"]}
    ],
    "state_dimensions": [
        {"id": "ci", "exclusive": true, "states": [{"id": "passed", "label": "ci-passed"}]},
        {"id": "review", "exclusive": true, "states": [{"id": "approved", "label": "review-approved"}]},
        {"id": "testing", "exclusive": true, "states": [{"id": "passed", "label": "testing-passed"}]},
        {"id": "merge", "exclusive": true, "states": [{"id": "ready", "label": "merge-ready"}]}
    ],
    "transitions": [
        {"id": "record_ci_pass", "artifact": "implementation_pr", "roles": ["ci"], "effects": [
            {"kind": "add_label", "label": "ci-passed"}
        ]},
        {"id": "approve_review", "artifact": "implementation_pr", "roles": ["reviewer"], "effects": [
            {"kind": "add_label", "label": "review-approved"}
        ]},
        {"id": "record_test_pass", "artifact": "implementation_pr", "roles": ["tester"], "effects": [
            {"kind": "add_label", "label": "testing-passed"}
        ]},
        {"id": "approve_merge", "artifact": "implementation_pr", "roles": ["owner"],
            "requires_gates": ["ci_gate", "review_gate", "testing_gate"], "effects": [
            {"kind": "add_label", "label": "merge-ready"}
        ]}
    ],
    "gates": [
        {"id": "ci_gate", "satisfied_by": ["record_ci_pass"]},
        {"id": "review_gate", "satisfied_by": ["approve_review"]},
        {"id": "testing_gate", "satisfied_by": ["record_test_pass"]}
    ]
}"#;

fn code_issue_input() -> CreateIssue {
    CreateIssue {
        title: "Add login flow".into(),
        body: "Implements login.".into(),
        labels: vec!["code".into(), "ready".into()],
        assignees: Vec::<UserId>::new(),
    }
}

fn implementation_pr_input(repo: &RepositoryId) -> CreatePullRequest {
    CreatePullRequest {
        title: "Implement login".into(),
        body: "Implements login.".into(),
        source: BranchRef {
            repository_id: repo.clone(),
            branch: "feature/login".into(),
        },
        target: BranchRef {
            repository_id: repo.clone(),
            branch: "main".into(),
        },
        labels: vec!["implementation".into()],
        assignees: Vec::<UserId>::new(),
    }
}

// Safety property 1: no duplicate artifact is created for one correlation key,
// even when an issue or pull-request create crashes after it lands in the
// backend.
#[test]
fn no_duplicate_artifact_is_created_for_a_correlation_key_after_a_crash() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    // Each create lands in the backend, then the call crashes before returning.
    let crash = CrashForge::new(
        forge,
        vec![
            Fault::after(ForgeOp::CreateIssue, 1),
            Fault::after(ForgeOp::CreatePullRequest, 1),
        ],
    );
    let workflow = workflow();
    let executor = Executor::new(&workflow, &crash);

    let crashed = block_on(executor.ensure_issue(&repo, "code-issue-42", code_issue_input()))
        .expect_err("the issue create crashes after it lands");
    assert!(matches!(crashed, ExecutionError::Backend { .. }));

    let after_crash =
        block_on(crash.inner().list_issues(&repo, IssueQuery::default())).expect("list issues");
    assert_eq!(
        after_crash.len(),
        1,
        "the crashed issue create left exactly one issue"
    );

    let retry = block_on(executor.ensure_issue(&repo, "code-issue-42", code_issue_input()))
        .expect("the issue retry resolves to the existing issue");
    assert!(!retry.was_created(), "no duplicate issue is created");
    let after_retry =
        block_on(crash.inner().list_issues(&repo, IssueQuery::default())).expect("list issues");
    assert_eq!(after_retry.len(), 1, "still exactly one issue");

    let crashed = block_on(executor.ensure_pull_request(
        &repo,
        "implementation-pr-42",
        implementation_pr_input(&repo),
    ))
    .expect_err("the pull-request create crashes after it lands");
    assert!(matches!(crashed, ExecutionError::Backend { .. }));

    let after_crash = block_on(
        crash
            .inner()
            .list_pull_requests(&repo, PullRequestQuery::default()),
    )
    .expect("list pull requests");
    assert_eq!(
        after_crash.len(),
        1,
        "the crashed PR create left exactly one pull request"
    );

    let retry = block_on(executor.ensure_pull_request(
        &repo,
        "implementation-pr-42",
        implementation_pr_input(&repo),
    ))
    .expect("the PR retry resolves to the existing pull request");
    assert!(!retry.was_created(), "no duplicate PR is created");
    let after_retry = block_on(
        crash
            .inner()
            .list_pull_requests(&repo, PullRequestQuery::default()),
    )
    .expect("list pull requests");
    assert_eq!(after_retry.len(), 1, "still exactly one pull request");
}

// Safety property 2: an exclusive claim never holds two active leases at once.
#[test]
fn an_exclusive_claim_never_has_two_active_leases() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let number = create_issue(&forge, &repo, &["code", "in-progress"], "Implement login.");
    let target = ArtifactSource::Issue { number };
    let manager = LeaseManager::new(&forge, LeasePolicy::new(Duration::minutes(30)));

    // Worker A claims first.
    block_on(manager.acquire(
        &repo,
        target,
        RoleId::new("engineer"),
        "run-a",
        ts("2026-05-29T00:00:00Z"),
    ))
    .expect("worker A claims");

    // Interleaved: worker B tries to claim the same issue while A's lease is live
    // and is rejected.
    let conflict = block_on(manager.acquire(
        &repo,
        target,
        RoleId::new("engineer"),
        "run-b",
        ts("2026-05-29T00:10:00Z"),
    ))
    .expect_err("a live lease cannot be taken by a second worker");
    assert!(matches!(
        conflict,
        LeaseError::Conflict(LeaseConflict::HeldByOther { .. })
    ));

    // The artifact records exactly one lease, held by A: the metadata cannot
    // represent two concurrent claims.
    let metadata = parse_metadata_block(&issue_body(&forge, &repo, number))
        .expect("metadata parses")
        .expect("metadata is present");
    let lease = metadata.lease.expect("a single lease is recorded");
    assert_eq!(lease.worker, "run-a", "the original holder keeps the claim");
}

// Safety property 3a: a merge is not authorized — and the pull request is not
// merged — until review and testing gates pass; once they do, the merge
// executes and projects the post-merge `landed`/`owner-pending` labels.
#[test]
fn a_merge_is_not_authorized_until_review_and_testing_gates_pass() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let workflow = workflow();
    let executor = Executor::new(&workflow, &forge);

    // Only review approved; testing still pending.
    let ungated = create_pr(&forge, &repo, &["implementation", "review-approved"], "");
    let blocked = block_on(executor.execute(
        &repo,
        ArtifactSource::PullRequest { number: ungated },
        &TransitionId::new("approve_merge"),
        &RoleId::new("owner"),
    ))
    .expect_err("a merge cannot be authorized before testing passes");
    assert!(matches!(blocked, ExecutionError::Precondition { .. }));
    assert!(
        !pr_labels(&forge, &repo, ungated).contains(&"merge-ready".to_string()),
        "no merge-ready label is set while a gate is unmet"
    );
    // No premature merge: the pull request is untouched.
    assert_eq!(pr_state(&forge, &repo, ungated), PullRequestState::Open);

    // Both gates met: review approved and testing passed.
    let gated = create_pr(
        &forge,
        &repo,
        &["implementation", "review-approved", "testing-passed"],
        "",
    );
    block_on(executor.execute(
        &repo,
        ArtifactSource::PullRequest { number: gated },
        &TransitionId::new("approve_merge"),
        &RoleId::new("owner"),
    ))
    .expect("a fully gated merge is authorized");
    // The pull request is merged and carries the post-merge projection, which
    // survives on the now-closed pull request.
    assert_eq!(pr_state(&forge, &repo, gated), PullRequestState::Merged);
    let labels = pr_labels(&forge, &repo, gated);
    assert!(labels.contains(&"merge-ready".to_string()));
    assert!(labels.contains(&"landed".to_string()));
    assert!(labels.contains(&"owner-pending".to_string()));
}

// Safety property 3c: the merge executes at most once. A crash that lands the
// merge but loses the response leaves post-merge labels unapplied; the retry
// detects the already-merged pull request, skips the merge, and finishes the
// projection without merging a second time.
#[test]
fn a_merge_executes_at_most_once_under_retry() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let number = create_pr(
        &forge,
        &repo,
        &["implementation", "review-approved", "testing-passed"],
        "",
    );
    // The merge lands in the backend, then the call crashes before returning.
    let crash = CrashForge::new(forge, vec![Fault::after(ForgeOp::MergePullRequest, 1)]);
    let workflow = workflow();
    let executor = Executor::new(&workflow, &crash);
    let pr = ArtifactSource::PullRequest { number };

    let crashed = block_on(executor.execute(
        &repo,
        pr,
        &TransitionId::new("approve_merge"),
        &RoleId::new("owner"),
    ))
    .expect_err("the merge crashes after it lands");
    assert!(matches!(crashed, ExecutionError::Backend { .. }));

    // The merge landed, but the post-merge labels did not yet.
    let after_crash = block_on(crash.inner().get_pull_request_by_number(&repo, number))
        .expect("lookup succeeds")
        .expect("pull request exists");
    assert_eq!(after_crash.state, PullRequestState::Merged);
    assert!(!after_crash.labels.contains(&"merge-ready".to_string()));

    // The retry skips the already-merged target and finishes the projection.
    block_on(executor.execute(
        &repo,
        pr,
        &TransitionId::new("approve_merge"),
        &RoleId::new("owner"),
    ))
    .expect("the retry completes the post-merge projection");
    assert_eq!(
        crash.count(ForgeOp::MergePullRequest),
        1,
        "the pull request is merged at most once"
    );
    let labels = pr_labels(crash.inner(), &repo, number);
    assert!(labels.contains(&"merge-ready".to_string()));
    assert!(labels.contains(&"landed".to_string()));
    assert!(labels.contains(&"owner-pending".to_string()));
}

// Safety property 3b: the same gate mechanism blocks a merge until CI, review,
// and testing all pass.
#[test]
fn the_merge_gate_mechanism_requires_ci_review_and_testing_together() {
    let spec: RawWorkflowSpec = serde_json::from_str(THREE_GATE).expect("three-gate json parses");
    let workflow = spec.validate().expect("three-gate workflow validates");
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let executor = Executor::new(&workflow, &forge);

    // Dropping any single gate blocks the merge.
    let missing_one = [
        ["implementation", "review-approved", "testing-passed"], // no CI
        ["implementation", "ci-passed", "testing-passed"],       // no review
        ["implementation", "ci-passed", "review-approved"],      // no testing
    ];
    for labels in missing_one {
        let number = create_pr(&forge, &repo, &labels, "");
        let error = block_on(executor.execute(
            &repo,
            ArtifactSource::PullRequest { number },
            &TransitionId::new("approve_merge"),
            &RoleId::new("owner"),
        ))
        .expect_err("a missing gate blocks the merge");
        assert!(
            matches!(error, ExecutionError::Precondition { .. }),
            "a missing gate is a precondition failure: {error:?}"
        );
        assert!(!pr_labels(&forge, &repo, number).contains(&"merge-ready".to_string()));
    }

    // All three gates satisfied: the merge is authorized.
    let number = create_pr(
        &forge,
        &repo,
        &[
            "implementation",
            "ci-passed",
            "review-approved",
            "testing-passed",
        ],
        "",
    );
    block_on(executor.execute(
        &repo,
        ArtifactSource::PullRequest { number },
        &TransitionId::new("approve_merge"),
        &RoleId::new("owner"),
    ))
    .expect("all three gates satisfied authorizes the merge");
    assert!(pr_labels(&forge, &repo, number).contains(&"merge-ready".to_string()));
}

// Safety property 4: a failed review gate returns the work to the engineer, and
// the return path is not available to the reviewer.
#[test]
fn a_failed_review_gate_returns_work_to_the_engineer() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let workflow = workflow();
    let executor = Executor::new(&workflow, &forge);

    let number = create_pr(&forge, &repo, &["implementation", "needs-review"], "");
    let pr = ArtifactSource::PullRequest { number };

    // The reviewer requests changes: the review gate fails.
    block_on(executor.execute(
        &repo,
        pr,
        &TransitionId::new("request_changes"),
        &RoleId::new("reviewer"),
    ))
    .expect("the reviewer can request changes");
    assert!(pr_labels(&forge, &repo, number).contains(&"review-changes-requested".to_string()));

    // The return path belongs to the engineer; the reviewer cannot perform it.
    let unauthorized = block_on(executor.execute(
        &repo,
        pr,
        &TransitionId::new("address_review_changes"),
        &RoleId::new("reviewer"),
    ))
    .expect_err("the reviewer cannot perform the engineer's return path");
    assert!(matches!(unauthorized, ExecutionError::Validation { .. }));

    // The engineer addresses the changes and sends it back for review.
    block_on(executor.execute(
        &repo,
        pr,
        &TransitionId::new("address_review_changes"),
        &RoleId::new("engineer"),
    ))
    .expect("the engineer addresses the requested changes");
    let labels = pr_labels(&forge, &repo, number);
    assert!(
        labels.contains(&"needs-review".to_string()),
        "work returns to review"
    );
    assert!(!labels.contains(&"review-changes-requested".to_string()));
}

// Safety property 5: expired in-progress work becomes visible for recovery.
#[test]
fn expired_in_progress_work_becomes_visible_for_recovery() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let workflow = workflow();
    let number = create_issue(&forge, &repo, &["code", "in-progress"], "Implement login.");
    let target = ArtifactSource::Issue { number };

    // An engineer claims the issue with a 30-minute lease, then "crashes": it
    // never heartbeats again.
    let manager = LeaseManager::new(&forge, LeasePolicy::new(Duration::minutes(30)));
    block_on(manager.acquire(
        &repo,
        target,
        RoleId::new("engineer"),
        "run-a",
        ts("2026-05-29T00:00:00Z"),
    ))
    .expect("the engineer claims the issue");

    let policy = DefaultRecoveryPolicy;
    let journal = InMemoryJournal::new();

    // Before expiry, the live lease is left alone.
    let quiet = block_on(workflow.reconciler(&policy).reconcile(
        &forge,
        &repo,
        &journal,
        ts("2026-05-29T00:20:00Z"),
    ))
    .expect("reconcile before expiry");
    assert!(quiet.is_clean(), "a live lease is not reconciled");

    // After expiry, the abandoned claim becomes visible and is requeued.
    let recovered = block_on(workflow.reconciler(&policy).reconcile(
        &forge,
        &repo,
        &journal,
        ts("2026-05-29T01:00:00Z"),
    ))
    .expect("reconcile after expiry");
    assert!(recovered.findings.iter().any(|finding| matches!(
        finding,
        ReconcileFinding::ExpiredLease { target: found, .. } if *found == target
    )));
    assert!(recovered
        .actions
        .contains(&RecoveryAction::RequeueLease { target }));
}

// Safety property 6: impossible label combinations are detected, not silently
// ignored — neither the executor nor the reconciler glosses over them.
#[test]
fn impossible_label_combinations_are_detected_not_silently_ignored() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let workflow = workflow();
    // `ready` and `in-progress` are two states of the exclusive code lifecycle.
    let number = create_issue(&forge, &repo, &["code", "ready", "in-progress"], "");
    let target = ArtifactSource::Issue { number };

    // The runtime refuses to transition the artifact rather than guessing a state.
    let executor = Executor::new(&workflow, &forge);
    let error = block_on(executor.execute(
        &repo,
        target,
        &TransitionId::new("claim_code"),
        &RoleId::new("engineer"),
    ))
    .expect_err("the runtime will not transition an impossible artifact");
    assert!(matches!(error, ExecutionError::Classification(_)));

    // And the reconciler surfaces it as an impossible state to escalate.
    let policy = DefaultRecoveryPolicy;
    let journal = InMemoryJournal::new();
    let report = block_on(workflow.reconciler(&policy).reconcile(
        &forge,
        &repo,
        &journal,
        ts("2026-05-29T00:00:00Z"),
    ))
    .expect("reconcile");
    assert!(report.findings.iter().any(|finding| matches!(
        finding,
        ReconcileFinding::ImpossibleState { dimension, .. } if dimension.as_str() == "code_lifecycle"
    )));
    assert!(report
        .actions
        .iter()
        .any(|action| matches!(action, RecoveryAction::Escalate { .. })));
}
