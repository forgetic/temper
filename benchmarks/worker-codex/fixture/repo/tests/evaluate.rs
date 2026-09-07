use delivery_policy::{evaluate_labels, parse_policy};

#[test]
fn reports_missing_required_labels() {
    let decision = evaluate_labels(&parse_policy("ready,review"), &["ready"]);
    assert!(!decision.accepted);
    assert_eq!(decision.missing, ["review"]);
}
