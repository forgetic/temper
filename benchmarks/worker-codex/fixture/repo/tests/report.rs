use delivery_policy::{evaluate_labels, parse_policy, render_decision};

#[test]
fn renders_a_rejection() {
    let decision = evaluate_labels(&parse_policy("ready,review"), &["ready"]);
    assert_eq!(render_decision(&decision), "missing: review");
}
