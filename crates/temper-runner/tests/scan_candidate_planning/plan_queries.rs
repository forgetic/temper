use super::*;

#[test]
fn normal_candidate_plan_keeps_open_queues_and_bounded_terminal_recovery() {
    let workflow = workflow_from_json(PLANNER_FIXTURE);
    let compiled = workflow.compile();
    let role = RoleId::new("engineer");

    // Normal role scans keep their open queue interest...
    let normal = candidate_query_plan(&workflow, &compiled, Some(&role), ScanMode::Normal);
    assert!(has_issue_query(
        &normal.issue_queries,
        IssueState::Open,
        &["ready", "urgent"]
    ));
    assert!(has_issue_query(
        &normal.issue_queries,
        IssueState::Open,
        &["ready", "bug"]
    ));
    assert!(has_pull_request_query(
        &normal.pull_request_queries,
        PullRequestState::Open,
        &[]
    ));
    // ...and now also carry the same bounded, label-scoped terminal recovery
    // queries as wake/audit so poll-only fleets converge terminal transitions.
    // Unlabelled closed history is still never requested.
    assert!(closed_issue_queries_have_labels(&normal.issue_queries));
    assert!(closed_pull_request_queries_have_labels(
        &normal.pull_request_queries
    ));
    assert!(has_issue_query(
        &normal.issue_queries,
        IssueState::Closed,
        &["ready"]
    ));

    // The mechanical automated hot-poll path stays open-only: no closed/merged
    // queries on the 1s poll.
    let automated = candidate_query_plan(&workflow, &compiled, None, ScanMode::Automated);
    assert!(
        !automated
            .issue_queries
            .iter()
            .any(|query| query.state == Some(IssueState::Closed))
    );
    assert!(!automated.pull_request_queries.iter().any(|query| {
        matches!(
            query.state,
            Some(PullRequestState::Closed | PullRequestState::Merged)
        )
    }));
}

#[test]
fn audit_candidate_plan_keeps_terminal_workflow_label_recovery_queries() {
    let workflow = workflow_from_json(PLANNER_FIXTURE);
    let compiled = workflow.compile();

    let audit = candidate_query_plan(&workflow, &compiled, None, ScanMode::Audit);
    assert!(closed_issue_queries_have_labels(&audit.issue_queries));
    assert!(closed_pull_request_queries_have_labels(
        &audit.pull_request_queries
    ));
    assert!(has_issue_query(
        &audit.issue_queries,
        IssueState::Closed,
        &["ready"]
    ));
    assert!(!has_issue_query(
        &audit.issue_queries,
        IssueState::Closed,
        &["code"]
    ));
    assert!(!has_pull_request_query(
        &audit.pull_request_queries,
        PullRequestState::Merged,
        &["implementation"]
    ));
}

#[test]
fn basic_delivery_landing_does_not_recover_merged_implementation_prs() {
    let workflow = workflow_from_json(BASIC_FIXTURE);
    let compiled = workflow.compile();

    // The basic landing queue intentionally has no `implementation` label filter:
    // the implementation_pr artifact kind still classifies open candidates, but
    // terminal recovery scans must not keep querying already-merged PRs merely
    // because they retain the implementation label.
    let audit = candidate_query_plan(&workflow, &compiled, None, ScanMode::Audit);
    assert!(!has_pull_request_query(
        &audit.pull_request_queries,
        PullRequestState::Closed,
        &["implementation"]
    ));
    assert!(!has_pull_request_query(
        &audit.pull_request_queries,
        PullRequestState::Merged,
        &["implementation"]
    ));
    assert!(!audit.pull_request_queries.iter().any(|query| {
        matches!(
            query.state,
            Some(PullRequestState::Closed | PullRequestState::Merged)
        )
    }));

    let automated = candidate_query_plan(&workflow, &compiled, None, ScanMode::Automated);
    assert!(has_pull_request_query(
        &automated.pull_request_queries,
        PullRequestState::Open,
        &[]
    ));
}

#[test]
fn wake_candidate_plan_keeps_terminal_recovery_but_preserves_role_queue_scope() {
    let workflow = workflow_from_json(PLANNER_FIXTURE);
    let compiled = workflow.compile();
    let role = RoleId::new("engineer");

    let wake = candidate_query_plan(&workflow, &compiled, Some(&role), ScanMode::Wake);
    assert!(has_issue_query(
        &wake.issue_queries,
        IssueState::Open,
        &["ready", "urgent"]
    ));
    assert!(has_issue_query(
        &wake.issue_queries,
        IssueState::Open,
        &["ready", "bug"]
    ));
    assert!(has_issue_query(
        &wake.issue_queries,
        IssueState::Closed,
        &["ready"]
    ));
    assert!(has_pull_request_query(
        &wake.pull_request_queries,
        PullRequestState::Open,
        &[]
    ));
    assert!(closed_issue_queries_have_labels(&wake.issue_queries));
    assert!(closed_pull_request_queries_have_labels(
        &wake.pull_request_queries
    ));

    let broad_wake = candidate_query_plan(&workflow, &compiled, None, ScanMode::Wake);
    assert_eq!(wake.issue_queries, broad_wake.issue_queries);
    assert_eq!(wake.pull_request_queries, broad_wake.pull_request_queries);
}

#[test]
fn automated_reference_plan_permits_single_default_kind_open_all_issue_query() {
    // The mechanical/automated scan is label-bounded, with one deliberate
    // exception: the reference workflow's default-kind `raw_intake` automation
    // queue carries no label, so raw human intake is discovered with a single
    // state-bounded open-all issue listing (open + summary, never closed
    // history). Landing's automation query stays label-bounded.
    let workflow = workflow_from_json(REFERENCE_FIXTURE);
    let compiled = workflow.compile();
    let plan = candidate_query_plan(&workflow, &compiled, None, ScanMode::Automated);

    let open_all_issue_queries: Vec<&IssueQuery> = plan
        .issue_queries
        .iter()
        .filter(|query| query.labels.is_empty())
        .collect();
    assert_eq!(
        open_all_issue_queries.len(),
        1,
        "exactly one default-kind open-all issue listing"
    );
    let open_all = open_all_issue_queries[0];
    assert_eq!(open_all.state, Some(IssueState::Open));
    assert_eq!(open_all.details, ItemListDetails::summary());

    // No terminal history is listed during active mechanical automation scans.
    assert!(
        plan.issue_queries
            .iter()
            .all(|query| query.state != Some(IssueState::Closed))
    );

    // The landing automation queue stays label-bounded.
    assert!(
        plan.pull_request_queries
            .iter()
            .all(|query| !query.labels.is_empty())
    );
    assert!(has_pull_request_query(
        &plan.pull_request_queries,
        PullRequestState::Open,
        &["landing"]
    ));
    assert!(!plan.pull_request_queries.iter().any(|query| {
        matches!(
            query.state,
            Some(PullRequestState::Closed | PullRequestState::Merged)
        )
    }));
}
