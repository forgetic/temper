//! Tests for bounded reconciliation loading.

mod support;

use support::crash::{CrashForge, ForgeOp};
use support::{
    TestRoot, add_issue_dependency, block_on, close_issue, create_issue, create_pr, new_repo, ts,
    workflow,
};
use temper_forge::{
    Forge, IssueQuery, IssueState, ItemListDetails, ItemNumber, PullRequestQuery, PullRequestState,
    PullRequestUpdateState, RepositoryId, UpdatePullRequest,
};
use temper_workflow::{
    ArtifactSource, CommandId, CommandJournal, CommandRecord, CommandState, DefaultRecoveryPolicy,
    InMemoryJournal, Postcondition, ReconcileFinding, RecoveryAction, RoleId, TransitionId,
    WorkflowEffect, reconciliation_candidate_query_plan,
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
fn candidate_plan_uses_workflow_labels_explicit_states_and_summary_details() {
    let workflow = workflow();
    let plan = reconciliation_candidate_query_plan(&workflow);
    let label_count = workflow.labels().len();

    let issue_terminal_count = 8;
    let pull_request_terminal_count = 9;

    assert_eq!(plan.issue_queries.len(), label_count + issue_terminal_count);
    assert_eq!(
        plan.pull_request_queries.len(),
        label_count + pull_request_terminal_count * 2
    );
    assert_eq!(
        count_issue_state(&plan.issue_queries, IssueState::Open),
        label_count
    );
    assert_eq!(
        count_issue_state(&plan.issue_queries, IssueState::Closed),
        issue_terminal_count
    );
    assert_eq!(
        count_pull_request_state(&plan.pull_request_queries, PullRequestState::Open),
        label_count
    );
    assert_eq!(
        count_pull_request_state(&plan.pull_request_queries, PullRequestState::Closed),
        pull_request_terminal_count
    );
    assert_eq!(
        count_pull_request_state(&plan.pull_request_queries, PullRequestState::Merged),
        pull_request_terminal_count
    );
    assert!(!has_pull_request_query(
        &plan.pull_request_queries,
        PullRequestState::Merged,
        "implementation"
    ));
    assert!(has_pull_request_query(
        &plan.pull_request_queries,
        PullRequestState::Merged,
        "landed"
    ));
    assert!(plan.issue_queries.iter().all(is_bounded_issue_query));
    assert!(
        plan.pull_request_queries
            .iter()
            .all(is_bounded_pull_request_query)
    );
}

#[test]
fn bounded_reconciliation_ignores_closed_unlabelled_history() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let workflow = workflow();
    let policy = DefaultRecoveryPolicy;
    let journal = InMemoryJournal::new();

    let empty_report = block_on(workflow.reconciler(&policy).reconcile(
        &forge,
        &repo,
        &journal,
        ts("2026-05-29T00:05:00Z"),
    ))
    .expect("empty repository reconciles");
    assert!(empty_report.is_clean());

    for _ in 0..20 {
        let issue = create_issue(&forge, &repo, &[], "");
        close_issue(&forge, &repo, issue);
        let pull_request = create_pr(&forge, &repo, &[], "");
        close_pull_request(&forge, &repo, pull_request);
    }

    let crash = CrashForge::new(forge.clone(), vec![]);
    let noisy_report = block_on(workflow.reconciler(&policy).reconcile(
        &crash,
        &repo,
        &journal,
        ts("2026-05-29T00:05:00Z"),
    ))
    .expect("noisy repository reconciles");

    assert_eq!(noisy_report, empty_report);
    assert_eq!(crash.count(ForgeOp::ListIssuesDefault), 0);
    assert_eq!(crash.count(ForgeOp::ListPullRequestsDefault), 0);
    assert_observed_bounded_summary_queries(&crash);
}

#[test]
fn dependency_gated_candidate_with_native_dependencies_is_reloaded_before_scan() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let dependency = create_issue(&forge, &repo, &["code", "ready"], "");
    let dependent = create_issue(&forge, &repo, &["code", "blocked"], "");
    add_issue_dependency(&forge, &repo, dependent, dependency);
    let workflow = workflow();
    let policy = DefaultRecoveryPolicy;
    let journal = InMemoryJournal::new();

    let crash = CrashForge::new(forge.clone(), vec![]);
    let report = block_on(workflow.reconciler(&policy).reconcile(
        &crash,
        &repo,
        &journal,
        ts("2026-05-29T00:05:00Z"),
    ))
    .expect("open dependency keeps dependent blocked without false diagnosis");

    assert!(
        report.is_clean(),
        "open native dependency should not be reported as missing: {:?}",
        report.findings
    );
    assert_eq!(crash.count(ForgeOp::ListIssuesDefault), 0);
    assert_observed_bounded_summary_queries(&crash);

    close_issue(&forge, &repo, dependency);
    let crash = CrashForge::new(forge.clone(), vec![]);
    let report = block_on(workflow.reconciler(&policy).reconcile(
        &crash,
        &repo,
        &journal,
        ts("2026-05-29T00:06:00Z"),
    ))
    .expect("landed dependency unblocks the dependent issue");

    assert_eq!(
        report.findings,
        vec![ReconcileFinding::DependenciesResolved {
            target: ArtifactSource::Issue { number: dependent },
            transition: TransitionId::new("mark_code_ready"),
        }]
    );
}

#[test]
fn bounded_candidate_discovery_finds_impossible_state_on_closed_artifact_once() {
    let root = TestRoot::new();
    let forge = root.forge();
    let repo = new_repo(&forge);
    let issue = create_issue(&forge, &repo, &["code", "ready", "blocked"], "");
    close_issue(&forge, &repo, issue);
    let workflow = workflow();
    let policy = DefaultRecoveryPolicy;
    let journal = InMemoryJournal::new();
    let crash = CrashForge::new(forge.clone(), vec![]);

    let report = block_on(workflow.reconciler(&policy).reconcile(
        &crash,
        &repo,
        &journal,
        ts("2026-05-29T00:05:00Z"),
    ))
    .expect("closed labelled artifact is a bounded candidate");

    let impossible = report
        .findings
        .iter()
        .filter(|finding| {
            matches!(
                finding,
                ReconcileFinding::ImpossibleState {
                    target: ArtifactSource::Issue { number },
                    ..
                } if *number == issue
            )
        })
        .count();
    assert_eq!(impossible, 1, "overlapping label queries must deduplicate");
    assert!(
        crash
            .issue_queries()
            .iter()
            .any(|query| query.state == Some(IssueState::Closed) && !query.labels.is_empty())
    );
    assert_observed_bounded_summary_queries(&crash);
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

fn count_issue_state(queries: &[IssueQuery], state: IssueState) -> usize {
    queries
        .iter()
        .filter(|query| query.state == Some(state))
        .count()
}

fn count_pull_request_state(queries: &[PullRequestQuery], state: PullRequestState) -> usize {
    queries
        .iter()
        .filter(|query| query.state == Some(state))
        .count()
}

fn has_pull_request_query(
    queries: &[PullRequestQuery],
    state: PullRequestState,
    label: &str,
) -> bool {
    queries
        .iter()
        .any(|query| query.state == Some(state) && has_single_label(&query.labels, label))
}

fn has_single_label(labels: &[String], label: &str) -> bool {
    labels.len() == 1 && labels[0] == label
}

fn is_bounded_issue_query(query: &IssueQuery) -> bool {
    query.state.is_some() && query.labels.len() == 1 && query.details == ItemListDetails::summary()
}

fn is_bounded_pull_request_query(query: &PullRequestQuery) -> bool {
    query.state.is_some() && query.labels.len() == 1 && query.details == ItemListDetails::summary()
}

fn assert_observed_bounded_summary_queries<F: Forge>(crash: &CrashForge<F>) {
    assert!(crash.issue_queries().iter().all(is_bounded_issue_query));
    assert!(
        crash
            .pull_request_queries()
            .iter()
            .all(is_bounded_pull_request_query)
    );
}

fn close_pull_request<F: Forge + ?Sized>(forge: &F, repo: &RepositoryId, number: ItemNumber) {
    let pull_request = block_on(forge.get_pull_request_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("pull request exists");
    block_on(forge.update_pull_request(
        &pull_request.id,
        UpdatePullRequest {
            state: Some(PullRequestUpdateState::Closed),
            ..UpdatePullRequest::default()
        },
    ))
    .expect("pull request closes");
}
