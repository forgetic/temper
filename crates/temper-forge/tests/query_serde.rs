use temper_forge::{IssueQuery, PullRequestQuery};

#[test]
fn query_defaults_have_no_body_filter() {
    assert_eq!(IssueQuery::default().body_contains, None);
    assert_eq!(PullRequestQuery::default().body_contains, None);
}

#[test]
fn issue_query_deserializes_old_shape_without_body_filter() {
    let query: IssueQuery = serde_json::from_str(r#"{"labels":[]}"#).unwrap();
    assert_eq!(query.body_contains, None);
    assert!(query.details.dependencies);
}

#[test]
fn pull_request_query_deserializes_old_shape_without_body_filter() {
    let query: PullRequestQuery = serde_json::from_str(r#"{"labels":[]}"#).unwrap();
    assert_eq!(query.body_contains, None);
    assert!(query.details.dependencies);
}

#[test]
fn default_query_serialization_omits_body_filter() {
    let issue = serde_json::to_value(IssueQuery::default()).unwrap();
    assert!(issue.get("body_contains").is_none());

    let pull = serde_json::to_value(PullRequestQuery::default()).unwrap();
    assert!(pull.get("body_contains").is_none());
}
