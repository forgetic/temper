//! Tests for applying reconciler recovery actions through the runtime.
//!
//! [`reconciliation.rs`](super) proves the pure `scan`/`reconcile` *decisions*;
//! this file proves the [`Applier`] *applies* them through the existing
//! components: lease clears go through [`LeaseManager`], label repairs/unblocks
//! reuse the executor's idempotent label-apply path, and journal transitions go
//! through the [`CommandJournal`]. Every test is deterministic against the
//! in-memory backend with fixed timestamps.

mod support;

use chrono::Duration;
use support::crash::{CrashForge, ForgeOp};
use support::{
    TestRoot, add_issue_dependency, block_on, close_issue, create_issue, issue_labels, new_repo,
    ts, workflow,
};
use temper_workflow::{
    Applier, ArtifactSource, CommandId, CommandJournal, CommandRecord, CommandState,
    DefaultRecoveryPolicy, Executor, InMemoryJournal, LeaseManager, LeasePolicy, RecoveryAction,
    RoleId, TransitionId, WorkflowEffect, parse_metadata_block,
};

const ENGINEER: &str = "engineer";

fn ttl_policy() -> LeasePolicy {
    LeasePolicy::new(Duration::minutes(30))
}

/// Reads a command's current journal state.
fn command_state(journal: &InMemoryJournal, id: &str) -> CommandState {
    block_on(journal.get(&CommandId::new(id)))
        .expect("journal get")
        .expect("command exists")
        .state
}

#[test]
fn requeue_lease_clears_the_lease_through_the_manager() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let number = create_issue(&forge, &repo, &["code", "in-progress"], "");
    let target = ArtifactSource::Issue { number };
    let workflow = workflow();
    let policy = DefaultRecoveryPolicy;
    let manager = LeaseManager::new(&forge, ttl_policy());
    let executor = Executor::new(&workflow, &forge);
    let journal = InMemoryJournal::new();

    // A worker claims the issue, then abandons it: the lease expires.
    block_on(manager.acquire(
        &repo,
        target,
        RoleId::new(ENGINEER),
        "run-a",
        ts("2026-05-29T00:00:00Z"),
    ))
    .expect("claim");

    let report = block_on(workflow.reconciler(&policy).reconcile_deep_audit(
        &forge,
        &repo,
        &journal,
        ts("2026-05-29T01:00:00Z"),
    ))
    .expect("reconcile after expiry");
    assert!(
        report
            .actions
            .contains(&RecoveryAction::RequeueLease { target })
    );

    let applier = Applier::new(&executor, &manager, &journal);
    let outcome =
        block_on(applier.apply_report(&repo, &report, ts("2026-05-29T01:00:00Z"))).expect("apply");
    assert_eq!(
        outcome.applied,
        vec![RecoveryAction::RequeueLease { target }]
    );

    // The lease is gone from the metadata, so the artifact is back in its queue.
    let metadata = parse_metadata_block(&support::issue_body(&forge, &repo, number))
        .expect("metadata parses")
        .expect("metadata present");
    assert!(metadata.lease.is_none(), "the lease was force-cleared");

    // Re-scanning finds nothing more to do.
    let after = block_on(workflow.reconciler(&policy).reconcile_deep_audit(
        &forge,
        &repo,
        &journal,
        ts("2026-05-29T01:05:00Z"),
    ))
    .expect("reconcile again");
    assert!(after.is_clean(), "the requeued artifact needs no recovery");
}

#[test]
fn repair_realizes_pending_labels_and_reconciles_the_command() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let number = create_issue(&forge, &repo, &["code", "ready"], "");
    let target = ArtifactSource::Issue { number };
    let workflow = workflow();
    let policy = DefaultRecoveryPolicy;
    let manager = LeaseManager::new(&forge, ttl_policy());
    let executor = Executor::new(&workflow, &forge);
    let journal = InMemoryJournal::new();

    // A claim was journaled and marked applying, but its labels never landed.
    block_on(journal.append(CommandRecord::planned(
        CommandId::new("claim-1"),
        target,
        TransitionId::new("claim_code"),
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

    let report = block_on(workflow.reconciler(&policy).reconcile_deep_audit(
        &forge,
        &repo,
        &journal,
        ts("2026-05-29T00:05:00Z"),
    ))
    .expect("reconcile");

    let applier = Applier::new(&executor, &manager, &journal);
    block_on(applier.apply_report(&repo, &report, ts("2026-05-29T00:05:00Z"))).expect("apply");

    // The pending labels are realized, and the originating command is resolved.
    assert_eq!(
        issue_labels(&forge, &repo, number),
        vec!["code".to_string(), "in-progress".to_string()]
    );
    assert_eq!(command_state(&journal, "claim-1"), CommandState::Reconciled);

    let after = block_on(workflow.reconciler(&policy).reconcile_deep_audit(
        &forge,
        &repo,
        &journal,
        ts("2026-05-29T00:06:00Z"),
    ))
    .expect("reconcile again");
    assert!(after.is_clean(), "repaired work converges in one pass");
}

#[test]
fn unblock_realizes_labels_and_journals_a_completed_command() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let dependency = create_issue(&forge, &repo, &["code", "ready"], "");
    close_issue(&forge, &repo, dependency);
    let number = create_issue(&forge, &repo, &["code", "blocked"], "");
    add_issue_dependency(&forge, &repo, number, dependency);
    let workflow = workflow();
    let policy = DefaultRecoveryPolicy;
    let manager = LeaseManager::new(&forge, ttl_policy());
    let executor = Executor::new(&workflow, &forge);
    let journal = InMemoryJournal::new();

    let report = block_on(workflow.reconciler(&policy).reconcile_deep_audit(
        &forge,
        &repo,
        &journal,
        ts("2026-05-29T00:00:00Z"),
    ))
    .expect("reconcile");
    assert!(
        report
            .actions
            .iter()
            .any(|action| matches!(action, RecoveryAction::Unblock { .. }))
    );

    let applier = Applier::new(&executor, &manager, &journal);
    block_on(applier.apply_report(&repo, &report, ts("2026-05-29T00:00:00Z"))).expect("apply");

    // The block is cleared and the work is ready.
    assert_eq!(
        issue_labels(&forge, &repo, number),
        vec!["code".to_string(), "ready".to_string()]
    );
    // The applier journaled its own command so a crash mid-apply is recoverable.
    let unblock_id = format!("reconcile-unblock:issue-{number}:mark_code_ready");
    assert_eq!(
        command_state(&journal, &unblock_id),
        CommandState::Completed
    );

    // Once unblocked, the issue no longer admits the mechanical unblock.
    let after = block_on(workflow.reconciler(&policy).reconcile_deep_audit(
        &forge,
        &repo,
        &journal,
        ts("2026-05-29T00:01:00Z"),
    ))
    .expect("reconcile again");
    assert!(after.is_clean(), "an unblocked issue is not re-unblocked");
}

#[test]
fn mark_reconciled_flips_a_stale_command_state() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    // The labels already reflect the claim, so the command is only lagging.
    let number = create_issue(&forge, &repo, &["code", "in-progress"], "");
    let target = ArtifactSource::Issue { number };
    let workflow = workflow();
    let policy = DefaultRecoveryPolicy;
    let manager = LeaseManager::new(&forge, ttl_policy());
    let executor = Executor::new(&workflow, &forge);
    let journal = InMemoryJournal::new();

    block_on(journal.append(CommandRecord::planned(
        CommandId::new("claim-4"),
        target,
        TransitionId::new("claim_code"),
        RoleId::new(ENGINEER),
        vec![
            WorkflowEffect::RemoveLabel("ready".into()),
            WorkflowEffect::AddLabel("in-progress".into()),
        ],
        ts("2026-05-29T00:00:00Z"),
    )))
    .expect("append");
    block_on(journal.transition_state(
        &CommandId::new("claim-4"),
        CommandState::Applying,
        None,
        ts("2026-05-29T00:00:30Z"),
    ))
    .expect("applying");

    let report = block_on(workflow.reconciler(&policy).reconcile_deep_audit(
        &forge,
        &repo,
        &journal,
        ts("2026-05-29T00:05:00Z"),
    ))
    .expect("reconcile");

    let applier = Applier::new(&executor, &manager, &journal);
    let outcome =
        block_on(applier.apply_report(&repo, &report, ts("2026-05-29T00:05:00Z"))).expect("apply");
    assert_eq!(
        outcome.applied,
        vec![RecoveryAction::MarkReconciled {
            command: CommandId::new("claim-4"),
        }]
    );
    assert_eq!(command_state(&journal, "claim-4"), CommandState::Reconciled);
}

#[test]
fn escalate_is_recorded_advisory_and_never_mutates_state() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    // `ready` and `in-progress` are exclusive: an impossible combination.
    let number = create_issue(&forge, &repo, &["code", "ready", "in-progress"], "");
    let workflow = workflow();
    let policy = DefaultRecoveryPolicy;
    let manager = LeaseManager::new(&forge, ttl_policy());
    let executor = Executor::new(&workflow, &forge);
    let journal = InMemoryJournal::new();

    let report = block_on(workflow.reconciler(&policy).reconcile_deep_audit(
        &forge,
        &repo,
        &journal,
        ts("2026-05-29T00:00:00Z"),
    ))
    .expect("reconcile");

    let applier = Applier::new(&executor, &manager, &journal);
    let outcome =
        block_on(applier.apply_report(&repo, &report, ts("2026-05-29T00:00:00Z"))).expect("apply");

    assert!(outcome.applied.is_empty(), "escalation is not applied");
    assert!(
        matches!(
            outcome.advisory.as_slice(),
            [RecoveryAction::Escalate { .. }]
        ),
        "escalation is recorded as advisory for a human"
    );
    // The impossible labels are left exactly as they were: no silent mutation.
    assert_eq!(
        issue_labels(&forge, &repo, number),
        vec![
            "code".to_string(),
            "in-progress".to_string(),
            "ready".to_string()
        ]
    );
}

#[test]
fn re_applying_a_report_is_a_no_op() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let number = create_issue(&forge, &repo, &["code", "ready"], "");
    let target = ArtifactSource::Issue { number };
    let workflow = workflow();
    let policy = DefaultRecoveryPolicy;
    // Wrap the backend with a fault-free CrashForge purely to count writes.
    let crash = CrashForge::new(forge, vec![]);
    let manager = LeaseManager::new(&crash, ttl_policy());
    let executor = Executor::new(&workflow, &crash);
    let journal = InMemoryJournal::new();

    block_on(journal.append(CommandRecord::planned(
        CommandId::new("claim-1"),
        target,
        TransitionId::new("claim_code"),
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

    let report = block_on(workflow.reconciler(&policy).reconcile_deep_audit(
        &crash,
        &repo,
        &journal,
        ts("2026-05-29T00:05:00Z"),
    ))
    .expect("reconcile");

    let applier = Applier::new(&executor, &manager, &journal);
    // First apply realizes the repair with a single update.
    block_on(applier.apply_report(&repo, &report, ts("2026-05-29T00:05:00Z"))).expect("apply 1");
    assert_eq!(crash.count(ForgeOp::UpdateIssue), 1);
    assert_eq!(command_state(&journal, "claim-1"), CommandState::Reconciled);

    // Re-applying the same (now stale) report writes nothing more.
    block_on(applier.apply_report(&repo, &report, ts("2026-05-29T00:06:00Z"))).expect("apply 2");
    assert_eq!(
        crash.count(ForgeOp::UpdateIssue),
        1,
        "a second apply of the same report issues no further write"
    );
    assert_eq!(
        issue_labels(crash.inner(), &repo, number),
        vec!["code".to_string(), "in-progress".to_string()]
    );
    assert_eq!(command_state(&journal, "claim-1"), CommandState::Reconciled);
}

#[test]
fn the_scan_apply_loop_converges_to_a_clean_state() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let workflow = workflow();
    let policy = DefaultRecoveryPolicy;
    let manager = LeaseManager::new(&forge, ttl_policy());
    let executor = Executor::new(&workflow, &forge);
    let journal = InMemoryJournal::new();

    // Three independent problems in one repository: an expired lease, an
    // interrupted claim whose labels never landed, and a blocked issue whose
    // dependency has now landed.
    let leased = create_issue(&forge, &repo, &["code", "in-progress"], "");
    block_on(manager.acquire(
        &repo,
        ArtifactSource::Issue { number: leased },
        RoleId::new(ENGINEER),
        "run-a",
        ts("2026-05-29T00:00:00Z"),
    ))
    .expect("claim");

    let partial = create_issue(&forge, &repo, &["code", "ready"], "");
    block_on(journal.append(CommandRecord::planned(
        CommandId::new("claim-partial"),
        ArtifactSource::Issue { number: partial },
        TransitionId::new("claim_code"),
        RoleId::new(ENGINEER),
        vec![
            WorkflowEffect::RemoveLabel("ready".into()),
            WorkflowEffect::AddLabel("in-progress".into()),
        ],
        ts("2026-05-29T00:00:00Z"),
    )))
    .expect("append");
    block_on(journal.transition_state(
        &CommandId::new("claim-partial"),
        CommandState::Applying,
        None,
        ts("2026-05-29T00:00:30Z"),
    ))
    .expect("applying");

    let dependency = create_issue(&forge, &repo, &["code", "ready"], "");
    close_issue(&forge, &repo, dependency);
    let blocked = create_issue(&forge, &repo, &["code", "blocked"], "");
    add_issue_dependency(&forge, &repo, blocked, dependency);

    let applier = Applier::new(&executor, &manager, &journal);
    let mut iterations = 0;
    loop {
        let report = block_on(workflow.reconciler(&policy).reconcile_deep_audit(
            &forge,
            &repo,
            &journal,
            ts("2026-05-29T01:00:00Z"),
        ))
        .expect("reconcile");
        if report.is_clean() {
            break;
        }
        let outcome = block_on(applier.apply_report(&repo, &report, ts("2026-05-29T01:00:00Z")))
            .expect("apply");
        // A loop that only ever advises (never applies) would not progress; the
        // default policy here always makes progress until clean.
        assert!(
            !outcome.applied.is_empty(),
            "each non-clean pass applies at least one action"
        );
        iterations += 1;
        assert!(iterations < 5, "the scan->apply loop converges quickly");
    }

    assert!(iterations >= 1, "there was real recovery work to converge");
}
