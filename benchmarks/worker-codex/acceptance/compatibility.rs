use delivery_policy::{Decision, Policy, evaluate_labels, parse_policy, render_decision};

#[test]
fn old_public_literals_and_exact_observation_matching_survive() {
    let policy = Policy {
        required: vec!["ready".into(), "review".into()],
    };
    assert_eq!(
        evaluate_labels(&policy, &["READY", " review "]),
        Decision {
            accepted: false,
            missing: vec!["ready".into(), "review".into()],
        }
    );
    assert_eq!(
        render_decision(&evaluate_labels(&policy, &["ready"])),
        "missing: review"
    );
    assert_eq!(
        render_decision(&evaluate_labels(&policy, &["ready", "review"])),
        "accepted"
    );
}

#[test]
fn legacy_duplicates_empties_and_ascii_normalization_survive() {
    let policy = parse_policy(" , Ready,READY, ÄBC,, review ");
    assert_eq!(policy.required, ["ready", "ready", "Äbc", "review"]);
    assert_eq!(
        evaluate_labels(&policy, &["review"]).missing,
        ["ready", "ready", "Äbc"]
    );
    assert_eq!(
        evaluate_labels(&parse_policy(" , ,"), &[]),
        Decision {
            accepted: true,
            missing: vec![],
        }
    );
}
