use delivery_policy::{Decision, Policy, evaluate_labels, parse_policy, render_decision};

#[test]
fn legacy_struct_literals_remain_supported() {
    let policy = Policy {
        required: vec!["ready".into()],
    };
    let decision = evaluate_labels(&policy, &["ready"]);
    assert_eq!(
        decision,
        Decision {
            accepted: true,
            missing: vec![]
        }
    );
    assert_eq!(render_decision(&decision), "accepted");
}

#[test]
fn legacy_parser_preserves_duplicates_and_ignores_empty_segments() {
    assert_eq!(
        parse_policy(" READY, ,Ready,,review ").required,
        ["ready", "ready", "review"]
    );
}

#[test]
fn legacy_observations_are_matched_exactly() {
    let decision = evaluate_labels(&parse_policy("ready,review"), &["READY", " review "]);
    assert_eq!(decision.missing, ["ready", "review"]);
    assert!(!decision.accepted);
}

#[test]
fn empty_legacy_policy_accepts() {
    assert_eq!(
        render_decision(&evaluate_labels(&parse_policy(" , "), &[])),
        "accepted"
    );
}
