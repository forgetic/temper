use temper_forge_model::{
    CandidateLabelSelection, CandidateLifecycle, IssueCandidateQuery, IssueQuery, ItemListDetails,
    PullRequestCandidateQuery, PullRequestQuery,
};

#[test]
fn query_defaults_have_no_body_filter() {
    assert_eq!(IssueQuery::default().body_contains, None);
    assert_eq!(IssueQuery::default().limit, None);
    assert_eq!(PullRequestQuery::default().body_contains, None);
    assert_eq!(PullRequestQuery::default().limit, None);
}

#[test]
fn issue_query_deserializes_old_shape_without_body_filter() {
    let query: IssueQuery = serde_json::from_str(r#"{"labels":[]}"#).unwrap();
    assert_eq!(query.body_contains, None);
    assert_eq!(query.limit, None);
    assert!(query.details.dependencies);
}

#[test]
fn pull_request_query_deserializes_old_shape_without_body_filter() {
    let query: PullRequestQuery = serde_json::from_str(r#"{"labels":[]}"#).unwrap();
    assert_eq!(query.body_contains, None);
    assert_eq!(query.limit, None);
    assert!(query.details.dependencies);
}

#[test]
fn default_query_serialization_omits_body_filter() {
    let issue = serde_json::to_value(IssueQuery::default()).unwrap();
    assert!(issue.get("body_contains").is_none());
    assert!(issue.get("limit").is_none());

    let pull = serde_json::to_value(PullRequestQuery::default()).unwrap();
    assert!(pull.get("body_contains").is_none());
    assert!(pull.get("limit").is_none());
}

#[test]
fn query_limit_round_trips_including_zero() {
    let issue: IssueQuery = serde_json::from_str(r#"{"labels":[],"limit":0}"#).unwrap();
    assert_eq!(issue.limit, Some(0));
    assert_eq!(serde_json::to_value(issue).unwrap()["limit"], 0);

    let pull: PullRequestQuery = serde_json::from_str(r#"{"labels":[],"limit":2}"#).unwrap();
    assert_eq!(pull.limit, Some(2));
    assert_eq!(serde_json::to_value(pull).unwrap()["limit"], 2);
}

#[test]
fn candidate_queries_default_to_open_unfiltered_summary() {
    let issue = IssueCandidateQuery::default();
    assert_eq!(issue.lifecycle, CandidateLifecycle::Open);
    assert_eq!(issue.labels, CandidateLabelSelection::Unfiltered);
    assert_eq!(issue.details, ItemListDetails::summary());
    assert_eq!(issue.page, None);

    let pull = PullRequestCandidateQuery::default();
    assert_eq!(pull.lifecycle, CandidateLifecycle::Open);
    assert_eq!(pull.labels, CandidateLabelSelection::Unfiltered);
    assert_eq!(pull.details, ItemListDetails::summary());
    assert_eq!(pull.page, None);

    let decoded_issue: IssueCandidateQuery = serde_json::from_str("{}").unwrap();
    let decoded_pull: PullRequestCandidateQuery = serde_json::from_str("{}").unwrap();
    assert_eq!(decoded_issue, issue);
    assert_eq!(decoded_pull, pull);
}

#[test]
fn candidate_any_of_is_non_empty_normalized_and_deduplicated() {
    assert_eq!(
        CandidateLabelSelection::any_of(vec!["ready", "code", "ready"]).unwrap(),
        CandidateLabelSelection::AnyOf(vec!["code".into(), "ready".into()])
    );
    assert!(CandidateLabelSelection::any_of(Vec::<String>::new()).is_err());
    assert!(
        CandidateLabelSelection::AnyOf(Vec::new())
            .normalized()
            .is_err()
    );
    assert!(serde_json::from_str::<CandidateLabelSelection>(r#"{"any_of":[]}"#).is_err());
}
