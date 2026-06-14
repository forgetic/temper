//! Crash-injection, retry, and restart-recovery tests (Phase 8).
//!
//! These drive the runtime against the [`CrashForge`](support::crash::CrashForge)
//! wrapper so a backend operation can fail deterministically before or after it
//! mutates the store. The tests prove the runtime guarantees from
//! `docs/reference/workflow-layer.md` and the robustness guarantees in
//! `docs/reference/robustness-guarantees.md`:
//!
//! - a crash before an effect leaves state intact and a retry completes;
//! - a crash after an effect lands does not double-apply on retry;
//! - a journaled command interrupted by a crash is repaired or marked
//!   reconciled after a restart;
//! - duplicated tool calls and interleaved workers claim an item at most once.
//!
//! Everything is deterministic: faults fire on fixed call counts and all time is
//! supplied through fixed timestamps, so there are no sleeps and no flakiness.

mod support;

use chrono::Duration;
use support::crash::{CrashForge, Fault, FaultError, FaultPoint, ForgeOp};
use support::{
    TestRoot, add_issue_dependency, block_on, close_issue, create_issue, issue_labels, new_repo,
    ts, workflow,
};
use temper_workflow::{
    Applier, ApplyError, ArtifactSource, CommandId, CommandJournal, CommandRecord, CommandState,
    DefaultRecoveryPolicy, ExecutionError, Executor, InMemoryJournal, LeaseManager, LeasePolicy,
    Postcondition, ReconcileFinding, RecoveryAction, RoleId, TransitionId, WorkflowEffect,
};

const CLAIM: &str = "claim_code";
const ENGINEER: &str = "engineer";

fn claimed() -> Vec<String> {
    vec!["code".to_string(), "in-progress".to_string()]
}

fn ready() -> Vec<String> {
    vec!["code".to_string(), "ready".to_string()]
}

#[test]
fn crash_before_an_effect_leaves_state_clean_and_a_retry_completes() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let number = create_issue(&forge, &repo, &["code", "ready"], "");
    // Fail the first label write before it touches the backend.
    let crash = CrashForge::new(forge, vec![Fault::before(ForgeOp::UpdateIssue, 1)]);
    let workflow = workflow();
    let executor = Executor::new(&workflow, &crash);
    let target = ArtifactSource::Issue { number };

    let error = block_on(executor.execute(
        &repo,
        target,
        &TransitionId::new(CLAIM),
        &RoleId::new(ENGINEER),
    ))
    .expect_err("the injected fault fails the claim");
    assert!(matches!(error, ExecutionError::Backend { .. }));
    assert_eq!(
        issue_labels(crash.inner(), &repo, number),
        ready(),
        "a crash before the write leaves the issue untouched"
    );

    // The fault was occurrence #1; the retry's write is occurrence #2 and is not
    // faulted, so the claim now completes cleanly.
    block_on(executor.execute(
        &repo,
        target,
        &TransitionId::new(CLAIM),
        &RoleId::new(ENGINEER),
    ))
    .expect("the retry claims the ready issue");
    assert_eq!(issue_labels(crash.inner(), &repo, number), claimed());
}

#[test]
fn crash_after_an_effect_lands_once_and_a_retry_does_not_double_apply() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let number = create_issue(&forge, &repo, &["code", "ready"], "");
    // The label write lands, then the call fails: the classic crash-after-effect.
    let crash = CrashForge::new(forge, vec![Fault::after(ForgeOp::UpdateIssue, 1)]);
    let workflow = workflow();
    let executor = Executor::new(&workflow, &crash);
    let target = ArtifactSource::Issue { number };

    let error = block_on(executor.execute(
        &repo,
        target,
        &TransitionId::new(CLAIM),
        &RoleId::new(ENGINEER),
    ))
    .expect_err("the call fails after the write lands");
    assert!(matches!(error, ExecutionError::Backend { .. }));
    assert_eq!(
        issue_labels(crash.inner(), &repo, number),
        claimed(),
        "the effect landed before the fault fired"
    );

    // A retry re-loads fresh state, sees `in-progress`, and refuses the claim as
    // a stale precondition rather than applying it twice.
    let retry = block_on(executor.execute(
        &repo,
        target,
        &TransitionId::new(CLAIM),
        &RoleId::new(ENGINEER),
    ))
    .expect_err("re-claiming an in-progress issue is stale");
    assert!(matches!(retry, ExecutionError::Precondition { .. }));
    assert_eq!(
        issue_labels(crash.inner(), &repo, number),
        claimed(),
        "the transition is still applied exactly once"
    );
}

#[test]
fn a_claim_is_applied_at_most_once_under_any_single_write_fault() {
    // A small deterministic simulator: crash the claim's single label write at
    // each fault point, retrying like a crash-looping worker, and assert the
    // issue is never left in a partial or double-applied state.
    for point in [FaultPoint::Before, FaultPoint::After] {
        let root = TestRoot::new();
        let forge = root.forge();
        let repo = new_repo(&forge);
        let number = create_issue(&forge, &repo, &["code", "ready"], "");
        let crash = CrashForge::new(
            forge,
            vec![Fault {
                op: ForgeOp::UpdateIssue,
                occurrence: 1,
                point,
                error: FaultError::Backend,
            }],
        );
        let workflow = workflow();
        let executor = Executor::new(&workflow, &crash);
        let target = ArtifactSource::Issue { number };

        let mut resolved = false;
        for _ in 0..3 {
            match block_on(executor.execute(
                &repo,
                target,
                &TransitionId::new(CLAIM),
                &RoleId::new(ENGINEER),
            )) {
                Ok(_) => {
                    resolved = true;
                    break;
                }
                // The injected crash; a real worker would retry.
                Err(ExecutionError::Backend { .. }) => continue,
                // A prior crashed attempt already landed the effect.
                Err(ExecutionError::Precondition { .. }) => {
                    resolved = true;
                    break;
                }
                Err(other) => panic!("unexpected error for {point:?}: {other:?}"),
            }
        }
        assert!(resolved, "the claim eventually resolves for {point:?}");

        // Because the executor folds all label effects into one backend write,
        // the issue is always either fully claimed or untouched — never half.
        assert_eq!(
            issue_labels(crash.inner(), &repo, number),
            claimed(),
            "labels are coherent after a {point:?} crash"
        );
    }
}

#[test]
fn a_journaled_claim_that_crashes_before_the_write_is_repaired_after_restart() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let number = create_issue(&forge, &repo, &["code", "ready"], "");
    let crash = CrashForge::new(forge, vec![Fault::before(ForgeOp::UpdateIssue, 1)]);
    let workflow = workflow();
    let executor = Executor::new(&workflow, &crash);
    let journal = InMemoryJournal::new();
    let target = ArtifactSource::Issue { number };

    // A worker journals the claim, marks it applying, then crashes before the
    // write — and before recording any terminal journal state.
    let preview = block_on(executor.plan(
        &repo,
        target,
        &TransitionId::new(CLAIM),
        &RoleId::new(ENGINEER),
    ))
    .expect("the claim plans");
    block_on(journal.append(CommandRecord::planned(
        CommandId::new("claim-1"),
        target,
        preview.transition.clone(),
        preview.role.clone(),
        preview.effects.clone(),
        ts("2026-05-29T00:00:00Z"),
    )))
    .expect("journal the intent");
    block_on(journal.transition_state(
        &CommandId::new("claim-1"),
        CommandState::Applying,
        None,
        ts("2026-05-29T00:00:30Z"),
    ))
    .expect("mark applying");
    let crashed = block_on(executor.execute(
        &repo,
        target,
        &TransitionId::new(CLAIM),
        &RoleId::new(ENGINEER),
    ))
    .expect_err("the injected fault crashes the apply");
    assert!(matches!(crashed, ExecutionError::Backend { .. }));

    // Restart: a fresh runtime attaches to the same backend and journal. The
    // effect never landed, so the interrupted command is a partial transition to
    // repair.
    let policy = DefaultRecoveryPolicy;
    let restarted = journal.clone();
    let report = block_on(workflow.reconciler(&policy).reconcile_deep_audit(
        crash.inner(),
        &repo,
        &restarted,
        ts("2026-05-29T00:05:00Z"),
    ))
    .expect("reconcile loads state");

    assert_eq!(
        report.findings,
        vec![ReconcileFinding::PartialTransition {
            command: CommandId::new("claim-1"),
            target,
            pending: vec![
                Postcondition::LabelAbsent("ready".into()),
                Postcondition::LabelPresent("in-progress".into()),
            ],
        }]
    );
    assert_eq!(
        report.actions,
        vec![RecoveryAction::Repair {
            target,
            effects: vec![
                WorkflowEffect::RemoveLabel("ready".into()),
                WorkflowEffect::AddLabel("in-progress".into()),
            ],
        }]
    );
}

#[test]
fn a_journaled_claim_that_lands_before_a_crash_is_marked_reconciled_after_restart() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let number = create_issue(&forge, &repo, &["code", "ready"], "");
    let crash = CrashForge::new(forge, vec![Fault::after(ForgeOp::UpdateIssue, 1)]);
    let workflow = workflow();
    let executor = Executor::new(&workflow, &crash);
    let journal = InMemoryJournal::new();
    let target = ArtifactSource::Issue { number };

    let preview = block_on(executor.plan(
        &repo,
        target,
        &TransitionId::new(CLAIM),
        &RoleId::new(ENGINEER),
    ))
    .expect("the claim plans");
    block_on(journal.append(CommandRecord::planned(
        CommandId::new("claim-1"),
        target,
        preview.transition.clone(),
        preview.role.clone(),
        preview.effects.clone(),
        ts("2026-05-29T00:00:00Z"),
    )))
    .expect("journal the intent");
    block_on(journal.transition_state(
        &CommandId::new("claim-1"),
        CommandState::Applying,
        None,
        ts("2026-05-29T00:00:30Z"),
    ))
    .expect("mark applying");
    // The write lands, then the worker crashes before recording completion.
    let crashed = block_on(executor.execute(
        &repo,
        target,
        &TransitionId::new(CLAIM),
        &RoleId::new(ENGINEER),
    ))
    .expect_err("the call fails after the write lands");
    assert!(matches!(crashed, ExecutionError::Backend { .. }));
    assert_eq!(issue_labels(crash.inner(), &repo, number), claimed());

    // Restart: the effects already landed, so only the journal status lags. The
    // command is stale and is marked reconciled rather than re-applied.
    let policy = DefaultRecoveryPolicy;
    let restarted = journal.clone();
    let report = block_on(workflow.reconciler(&policy).reconcile_deep_audit(
        crash.inner(),
        &repo,
        &restarted,
        ts("2026-05-29T00:05:00Z"),
    ))
    .expect("reconcile loads state");

    assert_eq!(
        report.findings,
        vec![ReconcileFinding::StaleCommand {
            command: CommandId::new("claim-1"),
            target,
            state: CommandState::Applying,
        }]
    );
    assert_eq!(
        report.actions,
        vec![RecoveryAction::MarkReconciled {
            command: CommandId::new("claim-1"),
        }]
    );
}

#[test]
fn duplicated_tool_calls_and_interleaved_workers_claim_an_item_at_most_once() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let number = create_issue(&forge, &repo, &["code", "ready"], "");
    let workflow = workflow();
    // One runtime authority serves both workers; each call re-loads fresh state.
    let executor = Executor::new(&workflow, &forge);
    let target = ArtifactSource::Issue { number };

    // Worker A (or the first of a duplicated pair of tool calls) wins the claim.
    let first = block_on(executor.execute(
        &repo,
        target,
        &TransitionId::new(CLAIM),
        &RoleId::new(ENGINEER),
    ))
    .expect("the first claim wins");
    assert_eq!(
        first.applied,
        vec![
            WorkflowEffect::RemoveLabel("ready".into()),
            WorkflowEffect::AddLabel("in-progress".into()),
        ]
    );

    // Worker B (or the duplicate call) re-loads fresh state, sees `in-progress`,
    // and is refused — the issue is not claimed twice.
    let second = block_on(executor.execute(
        &repo,
        target,
        &TransitionId::new(CLAIM),
        &RoleId::new(ENGINEER),
    ))
    .expect_err("the second claim sees fresh state and is refused");
    assert!(matches!(second, ExecutionError::Precondition { .. }));

    assert_eq!(issue_labels(&forge, &repo, number), claimed());
}

fn command_state(journal: &InMemoryJournal, id: &str) -> CommandState {
    block_on(journal.get(&CommandId::new(id)))
        .expect("journal get")
        .expect("command exists")
        .state
}

// Applying a `Repair` is retry-safe across a crash before or after the write:
// before, the labels are untouched and a retry repairs them; after, the labels
// already landed and a retry neither double-applies nor errors, then resolves
// the command. The reconciler reuses the executor's idempotent label-apply path.
#[test]
fn applying_a_repair_is_retry_safe_under_a_crash() {
    for point in [FaultPoint::Before, FaultPoint::After] {
        let root = TestRoot::new();
        let forge = root.forge();
        let repo = new_repo(&forge);
        let number = create_issue(&forge, &repo, &["code", "ready"], "");
        let target = ArtifactSource::Issue { number };
        let crash = CrashForge::new(
            forge,
            vec![Fault {
                op: ForgeOp::UpdateIssue,
                occurrence: 1,
                point,
                error: FaultError::Backend,
            }],
        );
        let workflow = workflow();
        let executor = Executor::new(&workflow, &crash);
        let manager = LeaseManager::new(&crash, LeasePolicy::new(Duration::minutes(30)));
        let journal = InMemoryJournal::new();

        // A claim was journaled and marked applying; its labels may or may not
        // have landed before the crash.
        block_on(journal.append(CommandRecord::planned(
            CommandId::new("claim-1"),
            target,
            TransitionId::new(CLAIM),
            RoleId::new(ENGINEER),
            vec![
                WorkflowEffect::RemoveLabel("ready".into()),
                WorkflowEffect::AddLabel("in-progress".into()),
            ],
            ts("2026-05-29T00:00:00Z"),
        )))
        .expect("append");
        block_on(journal.transition_state(
            &CommandId::new("claim-1"),
            CommandState::Applying,
            None,
            ts("2026-05-29T00:00:30Z"),
        ))
        .expect("applying");

        let report = block_on(
            workflow
                .reconciler(&DefaultRecoveryPolicy)
                .reconcile_deep_audit(&crash, &repo, &journal, ts("2026-05-29T00:05:00Z")),
        )
        .expect("reconcile");

        let applier = Applier::new(&executor, &manager, &journal);
        // The first apply crashes on the repair write.
        let crashed = block_on(applier.apply_report(&repo, &report, ts("2026-05-29T00:05:00Z")))
            .expect_err("the injected fault crashes the apply");
        assert!(matches!(
            crashed,
            ApplyError::Execution(ExecutionError::Backend { .. })
        ));
        // The command is not resolved while the repair is incomplete.
        assert_eq!(command_state(&journal, "claim-1"), CommandState::Applying);

        // A retry re-applies the same report. Occurrence #1 was the faulted
        // write; the retry either writes (after a before-crash) or no-ops (after
        // an after-crash), and never double-applies.
        block_on(applier.apply_report(&repo, &report, ts("2026-05-29T00:06:00Z")))
            .expect("the retry repairs cleanly");
        assert_eq!(
            issue_labels(crash.inner(), &repo, number),
            claimed(),
            "labels are coherent after a {point:?} crash"
        );
        assert_eq!(command_state(&journal, "claim-1"), CommandState::Reconciled);
    }
}

// Applying an `Unblock` is retry-safe across a crash before or after the write.
// The applier journals its own command, so a crash mid-apply leaves it
// incomplete and a retry finishes it without re-running a completed unblock.
#[test]
fn applying_an_unblock_is_retry_safe_under_a_crash() {
    for point in [FaultPoint::Before, FaultPoint::After] {
        let root = TestRoot::new();
        let forge = root.forge();
        let repo = new_repo(&forge);
        let dependency = create_issue(&forge, &repo, &["code", "ready"], "");
        close_issue(&forge, &repo, dependency);
        let number = create_issue(&forge, &repo, &["code", "blocked"], "");
        add_issue_dependency(&forge, &repo, number, dependency);
        let crash = CrashForge::new(
            forge,
            vec![Fault {
                op: ForgeOp::UpdateIssue,
                occurrence: 1,
                point,
                error: FaultError::Backend,
            }],
        );
        let workflow = workflow();
        let executor = Executor::new(&workflow, &crash);
        let manager = LeaseManager::new(&crash, LeasePolicy::new(Duration::minutes(30)));
        let journal = InMemoryJournal::new();

        let report = block_on(
            workflow
                .reconciler(&DefaultRecoveryPolicy)
                .reconcile_deep_audit(&crash, &repo, &journal, ts("2026-05-29T00:00:00Z")),
        )
        .expect("reconcile");

        let applier = Applier::new(&executor, &manager, &journal);
        let unblock_id = format!("reconcile-unblock:issue-{number}:mark_code_ready");

        let crashed = block_on(applier.apply_report(&repo, &report, ts("2026-05-29T00:00:00Z")))
            .expect_err("the injected fault crashes the unblock apply");
        assert!(matches!(
            crashed,
            ApplyError::Execution(ExecutionError::Backend { .. })
        ));
        // The applier journaled the unblock before mutating, so it is recoverable.
        assert_eq!(command_state(&journal, &unblock_id), CommandState::Applying);

        block_on(applier.apply_report(&repo, &report, ts("2026-05-29T00:01:00Z")))
            .expect("the retry unblocks cleanly");
        assert_eq!(
            issue_labels(crash.inner(), &repo, number),
            ready(),
            "the block is cleared exactly once after a {point:?} crash"
        );
        assert_eq!(
            command_state(&journal, &unblock_id),
            CommandState::Completed
        );
    }
}
