use delivery_policy::{
    Evaluation, PolicyOptions, Severity, Summary, Violation, evaluate_rule_policy,
    parse_rule_policy, render_rule_decision, summarize,
};

#[test]
fn full_workflow_reports_missing_rules_and_counts() {
    let policy = parse_rule_policy(
        "required:ready\nadvisory:docs\nrequired:review\nadvisory:changelog",
        PolicyOptions {
            promote_advisory: true,
            label_prefix: Some("gate:".into()),
        },
    )
    .unwrap();
    let result = evaluate_rule_policy(&policy, &["GATE:READY"]);
    assert_eq!(
        summarize(&result),
        Summary {
            required: 1,
            advisory: 2,
            total: 3
        }
    );
    assert_eq!(
        render_rule_decision(&result),
        "decision: rejected\nrequired: review\nadvisory: docs, changelog\ncounts: required=1, advisory=2, total=3"
    );
}

#[test]
fn accepted_report_can_include_advisories() {
    let policy = parse_rule_policy("advisory:docs", PolicyOptions::default()).unwrap();
    assert_eq!(
        render_rule_decision(&evaluate_rule_policy(&policy, &[])),
        "decision: accepted\nrequired: -\nadvisory: docs\ncounts: required=0, advisory=1, total=1"
    );
}

#[test]
fn empty_report_has_no_trailing_newline() {
    let result = Evaluation {
        accepted: true,
        required: vec![],
        advisory: vec![],
    };
    assert_eq!(
        summarize(&result),
        Summary {
            required: 0,
            advisory: 0,
            total: 0
        }
    );
    assert_eq!(
        render_rule_decision(&result),
        "decision: accepted\nrequired: -\nadvisory: -\ncounts: required=0, advisory=0, total=0"
    );
}

#[test]
fn report_respects_direct_evaluation_acceptance_and_vector_lengths() {
    let violation = Violation {
        severity: Severity::Required,
        label: "review".into(),
    };
    let result = Evaluation {
        accepted: true,
        required: vec![violation.clone(), violation],
        advisory: vec![],
    };
    assert_eq!(
        summarize(&result),
        Summary {
            required: 2,
            advisory: 0,
            total: 2
        }
    );
    assert_eq!(
        render_rule_decision(&result),
        "decision: accepted\nrequired: review, review\nadvisory: -\ncounts: required=2, advisory=0, total=2"
    );
    assert!(
        render_rule_decision(&Evaluation {
            accepted: false,
            required: vec![],
            advisory: vec![]
        })
        .starts_with("decision: rejected\n")
    );
}
