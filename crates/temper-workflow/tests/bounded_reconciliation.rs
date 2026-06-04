//! Tests for bounded reconciliation loading.

mod support;

use support::crash::{CrashForge, ForgeOp};
use support::{block_on, create_issue, create_pr, new_repo, ts, workflow, TestRoot};
use temper_forge::ItemNumber;
use temper_workflow::{
    ArtifactSource, CommandId, CommandJournal, CommandRecord, CommandState, DefaultRecoveryPolicy,
    InMemoryJournal, Postcondition, ReconcileFinding, RecoveryAction, RoleId, TransitionId,
    WorkflowEffect,
};

fn planned_record(id: &str, target: ArtifactSource, effects: Vec<WorkflowEffect>) -> CommandRecord {
    CommandRecord::planned(
        CommandId::new(id),
        target,
        TransitionId::new("claim_code"),
        RoleId::new("engineer"),
        effects,
        ts("2026-05-29T00:00:00Z"),
    )
}

fn append_applying(journal: &InMemoryJournal, record: CommandRecord) {
    let id = record.id.clone();
    block_on(journal.append(record)).expect("append");
    block_on(journal.transition_state(
        &id,
        CommandState::Applying,
        None,
        ts("2026-05-29T00:00:30Z"),
    ))
    .expect("applying");
}

#[test]
fn bounded_reconciliation_loads_unlabelled_journal_target_by_exact_get() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let number = create_issue(&forge, &repo, &[], "");
    let target = ArtifactSource::Issue { number };
    let crash = CrashForge::new(forge.clone(), vec![]);
    let workflow = workflow();
    let policy = DefaultRecoveryPolicy;
    let journal = InMemoryJournal::new();

    append_applying(
        &journal,
        planned_record(
            "claim-unlabelled",
            target,
            vec![WorkflowEffect::AddLabel("in-progress".into())],
        ),
    );

    let report = block_on(workflow.reconciler(&policy).reconcile(
        &crash,
        &repo,
        &journal,
        ts("2026-05-29T00:05:00Z"),
    ))
    .expect("bounded reconcile loads exact target");

    assert_eq!(crash.count(ForgeOp::GetIssueByNumber), 1);
    assert_eq!(crash.count(ForgeOp::ListIssuesDefault), 0);
    assert_eq!(crash.count(ForgeOp::ListPullRequestsDefault), 0);
    assert!(
        report.findings.iter().any(|finding| matches!(
            finding,
            ReconcileFinding::PartialTransition { command, target: found, pending }
                if command == &CommandId::new("claim-unlabelled")
                    && *found == target
                    && pending == &vec![Postcondition::LabelPresent("in-progress".into())]
        )),
        "loading the unlabelled target keeps the command partial instead of treating it as stale"
    );
    assert!(report.actions.iter().any(|action| matches!(
        action,
        RecoveryAction::Repair { target: found, effects }
            if *found == target && effects == &vec![WorkflowEffect::AddLabel("in-progress".into())]
    )));
}

#[test]
fn bounded_reconciliation_missing_journal_target_is_stale_without_default_lists() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let target = ArtifactSource::Issue {
        number: ItemNumber::new(404),
    };
    let crash = CrashForge::new(forge.clone(), vec![]);
    let workflow = workflow();
    let policy = DefaultRecoveryPolicy;
    let journal = InMemoryJournal::new();

    append_applying(
        &journal,
        planned_record(
            "claim-missing",
            target,
            vec![WorkflowEffect::AddLabel("in-progress".into())],
        ),
    );

    let report = block_on(workflow.reconciler(&policy).reconcile_bounded(
        &crash,
        &repo,
        &journal,
        Vec::new(),
        ts("2026-05-29T00:05:00Z"),
    ))
    .expect("bounded reconcile treats missing exact target as absent");

    assert_eq!(crash.count(ForgeOp::GetIssueByNumber), 1);
    assert_eq!(crash.count(ForgeOp::ListIssuesDefault), 0);
    assert_eq!(crash.count(ForgeOp::ListPullRequestsDefault), 0);
    assert_eq!(
        report.findings,
        vec![ReconcileFinding::StaleCommand {
            command: CommandId::new("claim-missing"),
            target,
            state: CommandState::Applying,
        }]
    );
}

#[test]
fn exact_journal_snapshots_are_deduplicated_and_ordered_deterministically() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let issue = create_issue(&forge, &repo, &[], "");
    let pull_request = create_pr(&forge, &repo, &[], "");
    assert_eq!(
        issue, pull_request,
        "reference backend numbers collide by type"
    );
    let crash = CrashForge::new(forge.clone(), vec![]);
    let workflow = workflow();
    let policy = DefaultRecoveryPolicy;
    let reconciler = workflow.reconciler(&policy);
    let records = vec![
        planned_record(
            "pr-first",
            ArtifactSource::PullRequest {
                number: pull_request,
            },
            Vec::new(),
        ),
        planned_record(
            "issue-second",
            ArtifactSource::Issue { number: issue },
            Vec::new(),
        ),
        planned_record(
            "issue-duplicate",
            ArtifactSource::Issue { number: issue },
            Vec::new(),
        ),
    ];

    let snapshots = block_on(reconciler.load_incomplete_journal_snapshots(&crash, &repo, &records))
        .expect("exact snapshots load");

    assert_eq!(crash.count(ForgeOp::GetIssueByNumber), 1);
    assert_eq!(crash.count(ForgeOp::GetPullRequestByNumber), 1);
    assert_eq!(
        snapshots
            .iter()
            .map(|snapshot| snapshot.source)
            .collect::<Vec<_>>(),
        vec![
            ArtifactSource::Issue { number: issue },
            ArtifactSource::PullRequest {
                number: pull_request,
            },
        ]
    );
}
