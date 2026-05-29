//! Tests for reconciliation of Forge artifacts and the command journal (Phase 7).
//!
//! The pure-scan tests assert the deterministic findings and policy-chosen
//! actions for expired leases, impossible states, partial transitions, and
//! stale commands. The end-to-end test exercises [`Reconciler::reconcile`]
//! against the in-memory backend and an in-memory journal, which is how a
//! restarted runtime rediscovers interrupted work.

mod support;

use harness_forge::ItemNumber;
use harness_workflow::{
    render_metadata_block, ArtifactKindId, ArtifactSnapshot, ArtifactSource, CommandId,
    CommandJournal, CommandRecord, CommandState, DefaultRecoveryPolicy, DependencyStatus,
    InMemoryJournal, Lease, Postcondition, ReconcileFinding, RecoveryAction, RecoveryPolicy,
    RoleId, StateDimensionId, StateId, TransitionId, WorkflowEffect, WorkflowMetadata,
};
use support::{block_on, create_issue, new_repo, ts, workflow, TestRoot};

fn issue_source(number: u64) -> ArtifactSource {
    ArtifactSource::Issue {
        number: ItemNumber::new(number),
    }
}

fn lease(worker: &str, expires_at: &str) -> Lease {
    Lease {
        role: RoleId::new("engineer"),
        worker: worker.to_string(),
        claimed_at: ts("2026-05-29T00:00:00Z"),
        heartbeat_at: ts("2026-05-29T00:00:00Z"),
        expires_at: ts(expires_at),
    }
}

/// Body for a `code` issue carrying a lease in its metadata block.
fn leased_body(worker: &str, expires_at: &str) -> String {
    let metadata = WorkflowMetadata {
        kind: Some(ArtifactKindId::new("code")),
        lease: Some(lease(worker, expires_at)),
        ..WorkflowMetadata::default()
    };
    render_metadata_block(&metadata)
}

#[test]
fn expired_lease_is_requeued_by_default_policy() {
    let workflow = workflow();
    let policy = DefaultRecoveryPolicy;
    let snapshot = ArtifactSnapshot {
        source: issue_source(1),
        labels: vec!["code".into(), "in-progress".into()],
        body: leased_body("run-1", "2026-05-29T00:30:00Z"),
    };

    let report = workflow.reconciler(&policy).scan(
        &[snapshot],
        &[],
        &DependencyStatus::default(),
        ts("2026-05-29T01:00:00Z"),
    );

    assert_eq!(
        report.findings,
        vec![ReconcileFinding::ExpiredLease {
            target: issue_source(1),
            lease: lease("run-1", "2026-05-29T00:30:00Z"),
        }]
    );
    assert_eq!(
        report.actions,
        vec![RecoveryAction::RequeueLease {
            target: issue_source(1)
        }]
    );
}

/// A policy that escalates expired leases instead of requeuing them.
struct EscalatingPolicy;

impl RecoveryPolicy for EscalatingPolicy {
    fn on_expired_lease(&self, target: ArtifactSource, lease: &Lease) -> RecoveryAction {
        RecoveryAction::Escalate {
            target,
            reason: format!("lease held by `{}` expired", lease.worker),
        }
    }
}

#[test]
fn expired_lease_action_follows_the_policy_hook() {
    let workflow = workflow();
    let policy = EscalatingPolicy;
    let snapshot = ArtifactSnapshot {
        source: issue_source(7),
        labels: vec!["code".into(), "in-progress".into()],
        body: leased_body("run-9", "2026-05-29T00:30:00Z"),
    };

    let report = workflow.reconciler(&policy).scan(
        &[snapshot],
        &[],
        &DependencyStatus::default(),
        ts("2026-05-29T01:00:00Z"),
    );

    assert_eq!(
        report.actions,
        vec![RecoveryAction::Escalate {
            target: issue_source(7),
            reason: "lease held by `run-9` expired".to_string(),
        }],
        "the reconciler honors the custom policy hook"
    );
}

#[test]
fn live_lease_is_not_reconciled() {
    let workflow = workflow();
    let policy = DefaultRecoveryPolicy;
    let snapshot = ArtifactSnapshot {
        source: issue_source(1),
        labels: vec!["code".into(), "in-progress".into()],
        body: leased_body("run-1", "2026-05-29T02:00:00Z"),
    };

    let report = workflow
        .reconciler(&policy)
        // Before expiry: nothing to do.
        .scan(
            &[snapshot],
            &[],
            &DependencyStatus::default(),
            ts("2026-05-29T01:00:00Z"),
        );
    assert!(report.is_clean());
}

#[test]
fn impossible_label_combination_is_detected_deterministically() {
    let workflow = workflow();
    let policy = DefaultRecoveryPolicy;
    // `ready` and `in-progress` are two states of the exclusive code lifecycle.
    let snapshot = ArtifactSnapshot {
        source: issue_source(2),
        labels: vec!["code".into(), "ready".into(), "in-progress".into()],
        body: String::new(),
    };

    let report = workflow.reconciler(&policy).scan(
        &[snapshot],
        &[],
        &DependencyStatus::default(),
        ts("2026-05-29T00:00:00Z"),
    );

    assert_eq!(
        report.findings,
        vec![ReconcileFinding::ImpossibleState {
            target: issue_source(2),
            dimension: StateDimensionId::new("code_lifecycle"),
            states: vec![StateId::new("ready"), StateId::new("in_progress")],
        }]
    );
    assert!(matches!(
        report.actions.as_slice(),
        [RecoveryAction::Escalate { .. }]
    ));
}

#[test]
fn partial_transition_emits_repair_effects() {
    let workflow = workflow();
    let policy = DefaultRecoveryPolicy;
    // The journal says claim_code was applying, but the artifact still shows
    // `ready` and lacks `in-progress`: the effects never landed.
    let snapshot = ArtifactSnapshot {
        source: issue_source(3),
        labels: vec!["code".into(), "ready".into()],
        body: String::new(),
    };
    let mut record = CommandRecord::planned(
        CommandId::new("claim-3"),
        issue_source(3),
        TransitionId::new("claim_code"),
        RoleId::new("engineer"),
        vec![
            WorkflowEffect::RemoveLabel("ready".into()),
            WorkflowEffect::AddLabel("in-progress".into()),
        ],
        ts("2026-05-29T00:00:00Z"),
    );
    record.state = CommandState::Applying;

    let report = workflow.reconciler(&policy).scan(
        &[snapshot],
        &[record],
        &DependencyStatus::default(),
        ts("2026-05-29T00:05:00Z"),
    );

    assert_eq!(
        report.findings,
        vec![ReconcileFinding::PartialTransition {
            command: CommandId::new("claim-3"),
            target: issue_source(3),
            pending: vec![
                Postcondition::LabelAbsent("ready".into()),
                Postcondition::LabelPresent("in-progress".into()),
            ],
        }]
    );
    assert_eq!(
        report.actions,
        vec![RecoveryAction::Repair {
            target: issue_source(3),
            effects: vec![
                WorkflowEffect::RemoveLabel("ready".into()),
                WorkflowEffect::AddLabel("in-progress".into()),
            ],
        }]
    );
}

#[test]
fn already_applied_command_is_marked_reconciled() {
    let workflow = workflow();
    let policy = DefaultRecoveryPolicy;
    // The journal is mid-flight, but the artifact already reflects the effects:
    // `ready` is gone and `in-progress` is present.
    let snapshot = ArtifactSnapshot {
        source: issue_source(4),
        labels: vec!["code".into(), "in-progress".into()],
        body: String::new(),
    };
    let mut record = CommandRecord::planned(
        CommandId::new("claim-4"),
        issue_source(4),
        TransitionId::new("claim_code"),
        RoleId::new("engineer"),
        vec![
            WorkflowEffect::RemoveLabel("ready".into()),
            WorkflowEffect::AddLabel("in-progress".into()),
        ],
        ts("2026-05-29T00:00:00Z"),
    );
    record.state = CommandState::Applying;

    let report = workflow.reconciler(&policy).scan(
        &[snapshot],
        &[record],
        &DependencyStatus::default(),
        ts("2026-05-29T00:05:00Z"),
    );

    assert_eq!(
        report.findings,
        vec![ReconcileFinding::StaleCommand {
            command: CommandId::new("claim-4"),
            target: issue_source(4),
            state: CommandState::Applying,
        }]
    );
    assert_eq!(
        report.actions,
        vec![RecoveryAction::MarkReconciled {
            command: CommandId::new("claim-4")
        }]
    );
}

#[test]
fn terminal_commands_are_ignored() {
    let workflow = workflow();
    let policy = DefaultRecoveryPolicy;
    let snapshot = ArtifactSnapshot {
        source: issue_source(5),
        labels: vec!["code".into(), "ready".into()],
        body: String::new(),
    };
    let mut record = CommandRecord::planned(
        CommandId::new("claim-5"),
        issue_source(5),
        TransitionId::new("claim_code"),
        RoleId::new("engineer"),
        vec![WorkflowEffect::RemoveLabel("ready".into())],
        ts("2026-05-29T00:00:00Z"),
    );
    record.state = CommandState::Completed;

    let report = workflow.reconciler(&policy).scan(
        &[snapshot],
        &[record],
        &DependencyStatus::default(),
        ts("2026-05-29T00:05:00Z"),
    );
    assert!(
        report.is_clean(),
        "a completed command needs no reconciliation"
    );
}

#[test]
fn reconcile_loads_backend_state_and_finds_interrupted_work() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = workflow();
    let repo = new_repo(&forge);
    let number = create_issue(&forge, &repo, &["code", "ready"], "");
    let policy = DefaultRecoveryPolicy;

    // A worker journaled a claim and began applying it, then crashed before the
    // backend or the journal recorded completion.
    let journal = InMemoryJournal::new();
    block_on(journal.append(CommandRecord::planned(
        CommandId::new("claim-1"),
        ArtifactSource::Issue { number },
        TransitionId::new("claim_code"),
        RoleId::new("engineer"),
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

    // Restart: a fresh reconciler attaches to the same backend and journal.
    let restarted_journal = journal.clone();
    let report = block_on(workflow.reconciler(&policy).reconcile(
        &forge,
        &repo,
        &restarted_journal,
        &DependencyStatus::default(),
        ts("2026-05-29T00:05:00Z"),
    ))
    .expect("reconcile loads state");

    // The artifact still shows `ready` (effects never landed), so the command is
    // a partial transition the policy wants repaired.
    assert_eq!(
        report.findings,
        vec![ReconcileFinding::PartialTransition {
            command: CommandId::new("claim-1"),
            target: ArtifactSource::Issue { number },
            pending: vec![
                Postcondition::LabelAbsent("ready".into()),
                Postcondition::LabelPresent("in-progress".into()),
            ],
        }]
    );
    assert_eq!(
        report.actions,
        vec![RecoveryAction::Repair {
            target: ArtifactSource::Issue { number },
            effects: vec![
                WorkflowEffect::RemoveLabel("ready".into()),
                WorkflowEffect::AddLabel("in-progress".into()),
            ],
        }]
    );
}

/// Body for a `code` issue that depends on the given prerequisite item numbers.
fn dependent_body(dependencies: &[u64]) -> String {
    render_metadata_block(&WorkflowMetadata {
        kind: Some(ArtifactKindId::new("code")),
        dependencies: dependencies.iter().map(|n| ItemNumber::new(*n)).collect(),
        ..WorkflowMetadata::default()
    })
}

#[test]
fn blocked_code_issue_unblocks_only_after_dependencies_land() {
    let workflow = workflow();
    let policy = DefaultRecoveryPolicy;
    let snapshot = ArtifactSnapshot {
        source: issue_source(8),
        labels: vec!["code".into(), "blocked".into()],
        body: dependent_body(&[9]),
    };

    // Prerequisite #9 has not landed: the reconciler leaves the block in place.
    let quiet = workflow.reconciler(&policy).scan(
        std::slice::from_ref(&snapshot),
        &[],
        &DependencyStatus::default(),
        ts("2026-05-29T00:00:00Z"),
    );
    assert!(quiet.is_clean(), "a still-blocked issue is not unblocked");

    // Once #9 lands, the reconciler mechanically produces the unblock action.
    let landed = DependencyStatus::landed([ItemNumber::new(9)]);
    let report =
        workflow
            .reconciler(&policy)
            .scan(&[snapshot], &[], &landed, ts("2026-05-29T00:00:00Z"));
    assert_eq!(
        report.findings,
        vec![ReconcileFinding::DependenciesResolved {
            target: issue_source(8),
            transition: TransitionId::new("mark_code_ready"),
        }]
    );
    assert_eq!(
        report.actions,
        vec![RecoveryAction::Unblock {
            target: issue_source(8),
            effects: vec![
                WorkflowEffect::RemoveLabel("blocked".into()),
                WorkflowEffect::AddLabel("ready".into()),
            ],
        }]
    );
}
