use delivery_policy::{
    Evaluation, PolicyOptions, Rule, RulePolicy, Severity, Violation, evaluate_rule_policy,
    parse_rule_policy,
};

fn violation(severity: Severity, label: &str) -> Violation {
    Violation {
        severity,
        label: label.into(),
    }
}

#[test]
fn normalizes_observations_but_preserves_severity_group_order() {
    let policy = parse_rule_policy(
        "advisory:docs\nrequired:review\nadvisory:changelog\nrequired:ready\nrequired:tested",
        PolicyOptions::default(),
    )
    .unwrap();
    let before = policy.clone();
    let result = evaluate_rule_policy(&policy, &[" TESTED ", "tested", "surplus"]);
    assert_eq!(
        result,
        Evaluation {
            accepted: false,
            required: vec![
                violation(Severity::Required, "review"),
                violation(Severity::Required, "ready")
            ],
            advisory: vec![
                violation(Severity::Advisory, "docs"),
                violation(Severity::Advisory, "changelog")
            ],
        }
    );
    assert_eq!(policy, before);
}

#[test]
fn promotion_changes_acceptance_without_reclassifying_violations() {
    for promote_advisory in [false, true] {
        let policy = parse_rule_policy(
            "required:ready\nadvisory:docs",
            PolicyOptions {
                promote_advisory,
                label_prefix: None,
            },
        )
        .unwrap();
        let result = evaluate_rule_policy(&policy, &["READY"]);
        assert_eq!(result.accepted, !promote_advisory);
        assert!(result.required.is_empty());
        assert_eq!(result.advisory, [violation(Severity::Advisory, "docs")]);
        assert!(evaluate_rule_policy(&policy, &["ready", "docs"]).accepted);
        assert!(!evaluate_rule_policy(&policy, &["docs"]).accepted);
    }
}

#[test]
fn normalized_prefix_is_optional_and_stripped_exactly_once() {
    let policy = parse_rule_policy(
        "required:ready\nrequired:review\nrequired:tag:docs",
        PolicyOptions {
            promote_advisory: false,
            label_prefix: Some(" TaG: ".into()),
        },
    )
    .unwrap();
    assert!(evaluate_rule_policy(&policy, &[" TAG: Ready ", " REVIEW ", "tag:tag:docs"]).accepted);
    let result = evaluate_rule_policy(&policy, &["x-tag:ready", "tag:review", "tag:docs"]);
    assert_eq!(
        result.required,
        [
            violation(Severity::Required, "ready"),
            violation(Severity::Required, "tag:docs")
        ]
    );
    let result = evaluate_rule_policy(&policy, &["tag:tag:ready", "review", "tag:tag:docs"]);
    assert_eq!(result.required, [violation(Severity::Required, "ready")]);
}

#[test]
fn absent_or_empty_prefix_does_not_change_matching() {
    for label_prefix in [None, Some("".into()), Some(" \t ".into())] {
        let policy = parse_rule_policy(
            "required:ready",
            PolicyOptions {
                promote_advisory: false,
                label_prefix,
            },
        )
        .unwrap();
        assert!(evaluate_rule_policy(&policy, &[" Ready "]).accepted);
        assert!(!evaluate_rule_policy(&policy, &["tag:ready"]).accepted);
    }
}

#[test]
fn direct_policies_keep_duplicate_rules_and_labels_are_not_renormalized() {
    let policy = RulePolicy {
        rules: vec![
            Rule {
                severity: Severity::Advisory,
                label: "tag:ready".into(),
            },
            Rule {
                severity: Severity::Required,
                label: "tag:ready".into(),
            },
            Rule {
                severity: Severity::Required,
                label: "tag:ready".into(),
            },
        ],
        options: PolicyOptions {
            promote_advisory: false,
            label_prefix: Some("tag:".into()),
        },
    };
    let result = evaluate_rule_policy(&policy, &["tag:ready"]);
    assert_eq!(
        result.required,
        vec![violation(Severity::Required, "tag:ready"); 2]
    );
    assert_eq!(
        result.advisory,
        [violation(Severity::Advisory, "tag:ready")]
    );
    assert!(evaluate_rule_policy(&policy, &["tag:tag:ready"]).accepted);
}

#[test]
fn empty_policies_accept_and_non_ascii_case_is_preserved() {
    let empty = parse_rule_policy(
        "",
        PolicyOptions {
            promote_advisory: true,
            label_prefix: None,
        },
    )
    .unwrap();
    assert_eq!(
        evaluate_rule_policy(&empty, &["extra"]),
        Evaluation {
            accepted: true,
            required: vec![],
            advisory: vec![],
        }
    );
    let policy = parse_rule_policy("required:ÄBC", PolicyOptions::default()).unwrap();
    assert!(evaluate_rule_policy(&policy, &[" ÄBc "]).accepted);
    assert!(!evaluate_rule_policy(&policy, &["äbc"]).accepted);
}
