//! Regression tests for bounded reconciliation candidate discovery.

mod support;

use support::{TestRoot, block_on, create_pr, new_repo, ts, workflow};
use temper_forge::{
    Forge, IssueQuery, IssueState, ItemNumber, MergeMethod, MergePullRequest, PullRequestQuery,
    PullRequestState, RepositoryId,
};
use temper_workflow::{
    ArtifactSource, CommandId, CommandJournal, CommandRecord, CommandState, DefaultRecoveryPolicy,
    InMemoryJournal, Postcondition, ReconcileFinding, RoleId, TransitionId, WorkflowEffect,
    reconciliation_candidate_query_plan,
};

fn has_issue_query(queries: &[IssueQuery], state: IssueState, label: &str) -> bool {
    queries
        .iter()
        .any(|query| query.state == Some(state) && has_single_label(&query.labels, label))
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

fn merge_pr<F: Forge + ?Sized>(forge: &F, repo: &RepositoryId, number: ItemNumber) {
    let pull_request = block_on(forge.get_pull_request_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("pull request exists");
    block_on(forge.merge_pull_request(
        &pull_request.id,
        MergePullRequest {
            method: MergeMethod::Squash,
            commit_title: None,
            commit_body: None,
        },
    ))
    .expect("pull request merges");
}

#[test]
fn bounded_reconciliation_skips_pure_terminal_identity_labels() {
    let workflow = workflow();
    let plan = reconciliation_candidate_query_plan(&workflow);

    assert!(has_issue_query(
        &plan.issue_queries,
        IssueState::Open,
        "epic"
    ));
    assert!(!has_issue_query(
        &plan.issue_queries,
        IssueState::Closed,
        "epic"
    ));
    assert!(has_pull_request_query(
        &plan.pull_request_queries,
        PullRequestState::Open,
        "implementation"
    ));
    assert!(!has_pull_request_query(
        &plan.pull_request_queries,
        PullRequestState::Closed,
        "implementation"
    ));
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
}

#[test]
fn bounded_reconciliation_loads_incomplete_merged_pr_by_exact_journal_target() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = workflow();
    let repo = new_repo(&forge);
    let number = create_pr(&forge, &repo, &["implementation"], "");
    merge_pr(&forge, &repo, number);
    let policy = DefaultRecoveryPolicy;
    let journal = InMemoryJournal::new();
    block_on(journal.append(CommandRecord::planned(
        CommandId::new("merge-1"),
        ArtifactSource::PullRequest { number },
        TransitionId::new("approve_merge"),
        RoleId::new("owner"),
        vec![
            WorkflowEffect::MergePullRequest,
            WorkflowEffect::AddLabel("landed".into()),
            WorkflowEffect::AddLabel("alignment".into()),
        ],
        ts("2026-05-29T00:00:00Z"),
    )))
    .expect("append");
    block_on(journal.transition_state(
        &CommandId::new("merge-1"),
        CommandState::Applying,
        None,
        ts("2026-05-29T00:00:30Z"),
    ))
    .expect("applying");

    let report = block_on(workflow.reconciler(&policy).reconcile(
        &forge,
        &repo,
        &journal,
        ts("2026-05-29T00:05:00Z"),
    ))
    .expect("bounded reconciliation loads exact journal target");

    assert_eq!(report.snapshot_count, 1);
    assert_eq!(
        report.findings,
        vec![ReconcileFinding::PartialTransition {
            command: CommandId::new("merge-1"),
            target: ArtifactSource::PullRequest { number },
            pending: vec![
                Postcondition::LabelPresent("landed".into()),
                Postcondition::LabelPresent("alignment".into()),
            ],
        }]
    );
}
