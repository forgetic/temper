use super::*;

#[test]
fn normal_and_automated_plans_use_constant_lifecycle_buckets() {
    let workflow = workflow_from_json(PLANNER_FIXTURE);
    let compiled = workflow.compile();
    let role = RoleId::new("engineer");

    let normal = candidate_query_plan(&workflow, &compiled, Some(&role), ScanMode::Normal);
    assert_eq!(normal.issue_queries.len(), 2);
    assert_eq!(normal.pull_request_queries.len(), 1);
    assert!(has_issue_query(
        &normal.issue_queries,
        CandidateLifecycle::Open,
        &["ready", "urgent", "bug"]
    ));
    assert!(has_pull_request_query(
        &normal.pull_request_queries,
        CandidateLifecycle::Open,
        &[]
    ));
    assert!(has_issue_query(
        &normal.issue_queries,
        CandidateLifecycle::Terminal,
        &["ready"]
    ));
    assert!(closed_issue_queries_have_labels(&normal.issue_queries));
    assert!(closed_pull_request_queries_have_labels(
        &normal.pull_request_queries
    ));

    let automated = candidate_query_plan(&workflow, &compiled, None, ScanMode::Automated);
    assert!(
        automated
            .issue_queries
            .iter()
            .all(|query| query.lifecycle == CandidateLifecycle::Open)
    );
    assert!(
        automated
            .pull_request_queries
            .iter()
            .all(|query| query.lifecycle == CandidateLifecycle::Open)
    );
}

#[test]
fn audit_uses_shared_bounded_terminal_interest() {
    let workflow = workflow_from_json(PLANNER_FIXTURE);
    let compiled = workflow.compile();
    let audit = candidate_query_plan(&workflow, &compiled, None, ScanMode::Audit);

    assert_eq!(audit.issue_queries.len(), 2);
    assert_eq!(audit.pull_request_queries.len(), 1);
    assert!(has_issue_query(
        &audit.issue_queries,
        CandidateLifecycle::Terminal,
        &["ready"]
    ));
    assert!(!has_issue_query(
        &audit.issue_queries,
        CandidateLifecycle::Terminal,
        &["code"]
    ));
    assert!(!has_pull_request_query(
        &audit.pull_request_queries,
        CandidateLifecycle::Terminal,
        &["implementation"]
    ));
    assert!(closed_issue_queries_have_labels(&audit.issue_queries));
    assert!(closed_pull_request_queries_have_labels(
        &audit.pull_request_queries
    ));
}

#[test]
fn basic_delivery_without_a_terminal_queue_emits_no_terminal_query() {
    let workflow = workflow_from_json(BASIC_FIXTURE);
    let compiled = workflow.compile();
    let audit = candidate_query_plan(&workflow, &compiled, None, ScanMode::Audit);

    assert!(
        audit
            .issue_queries
            .iter()
            .all(|query| query.lifecycle == CandidateLifecycle::Open)
    );
    assert!(
        audit
            .pull_request_queries
            .iter()
            .all(|query| query.lifecycle == CandidateLifecycle::Open)
    );

    let automated = candidate_query_plan(&workflow, &compiled, None, ScanMode::Automated);
    assert_eq!(automated.pull_request_queries.len(), 1);
    assert!(has_pull_request_query(
        &automated.pull_request_queries,
        CandidateLifecycle::Open,
        &["landing"]
    ));
    assert!(!has_pull_request_query(
        &automated.pull_request_queries,
        CandidateLifecycle::Open,
        &[]
    ));
}

#[test]
fn wake_preserves_role_scope_with_one_bucket_per_lifecycle() {
    let workflow = workflow_from_json(PLANNER_FIXTURE);
    let compiled = workflow.compile();
    let role = RoleId::new("engineer");

    let wake = candidate_query_plan(&workflow, &compiled, Some(&role), ScanMode::Wake);
    assert!(has_issue_query(
        &wake.issue_queries,
        CandidateLifecycle::Open,
        &["ready", "urgent", "bug"]
    ));
    assert!(has_issue_query(
        &wake.issue_queries,
        CandidateLifecycle::Terminal,
        &["ready"]
    ));
    assert!(has_pull_request_query(
        &wake.pull_request_queries,
        CandidateLifecycle::Open,
        &[]
    ));

    let broad_wake = candidate_query_plan(&workflow, &compiled, None, ScanMode::Wake);
    assert_eq!(wake.issue_queries, broad_wake.issue_queries);
    assert_eq!(wake.pull_request_queries, broad_wake.pull_request_queries);
}

#[test]
fn unfiltered_default_intake_dominates_labelled_open_interest() {
    let workflow = workflow_from_json(REFERENCE_FIXTURE);
    let compiled = workflow.compile();
    let plan = candidate_query_plan(&workflow, &compiled, None, ScanMode::Automated);

    let open_all_issue_queries: Vec<&IssueCandidateQuery> = plan
        .issue_queries
        .iter()
        .filter(|query| {
            query.lifecycle == CandidateLifecycle::Open
                && query.labels == CandidateLabelSelection::Unfiltered
        })
        .collect();
    assert_eq!(open_all_issue_queries.len(), 1);
    assert_eq!(
        open_all_issue_queries[0].details,
        ItemListDetails::summary()
    );
    assert!(
        plan.issue_queries
            .iter()
            .all(|query| query.lifecycle == CandidateLifecycle::Open)
    );

    assert_eq!(plan.pull_request_queries.len(), 1);
    assert!(has_pull_request_query(
        &plan.pull_request_queries,
        CandidateLifecycle::Open,
        &["landing"]
    ));
    assert!(
        plan.pull_request_queries
            .iter()
            .all(|query| query.lifecycle == CandidateLifecycle::Open)
    );
}
