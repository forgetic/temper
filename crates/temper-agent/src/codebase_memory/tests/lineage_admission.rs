use super::*;

#[test]
fn exact_selectors_resolve_before_provider_with_only_closed_values() {
    const PRIVATE_SELECTOR: &str = "crate::private::engine::run";
    let mut lineages = DecisionAnchorLineages::default();
    let root = lineages
        .record(
            &correlation(GraphCorrelationTargetKindV1::GraphQuery),
            &serde_json::json!({"query": "private discovery query"}),
            Some(&structured_parts(serde_json::json!({
                "qualified_name": PRIVATE_SELECTOR
            }))),
        )
        .unwrap();

    let implementation = lineages.resolve(
        GraphCorrelationToolV1::GetCodeSnippet.public_name(),
        &serde_json::json!({
            "qualified_name": PRIVATE_SELECTOR,
            "decision_evidence_kind": "implementation"
        }),
    );
    assert!(matches!(
        implementation,
        LineageAdmissionOutcome::Eligible(_)
    ));
    lineages
        .record_with_evidence_kind(
            &correlation(GraphCorrelationTargetKindV1::QualifiedName),
            &serde_json::json!({"qualified_name": PRIVATE_SELECTOR}),
            Some(&structured_parts(serde_json::json!({
                "qualified_name": PRIVATE_SELECTOR
            }))),
            Some(temper_protocol_activity::DecisionEvidenceKindV1::Implementation),
        )
        .unwrap();
    let admission = lineages.resolve(
        GraphCorrelationToolV1::TracePath.public_name(),
        &serde_json::json!({"function_name": "run", "direction": "inbound"}),
    );
    let LineageAdmissionOutcome::Eligible(admission) = admission else {
        panic!("exact registered selector must be eligible");
    };
    assert!(admission.matches_root(&root.root_binding));
    assert_eq!(
        admission.selector_kind(),
        DecisionAnchorTargetKindV1::FunctionName
    );
    assert_eq!(admission.tool_kind(), GraphCorrelationToolV1::TracePath);
    assert_eq!(admission.evidence_purpose(), None);
    let rendered = format!("{admission:?}");
    assert!(!rendered.contains(PRIVATE_SELECTOR));
    assert!(!rendered.contains(&root.root_binding));
}

#[test]
fn source_admission_requires_one_validated_wrapper_evidence_purpose() {
    let mut lineages = DecisionAnchorLineages::default();
    let root = lineages
        .record(
            &correlation(GraphCorrelationTargetKindV1::GraphQuery),
            &serde_json::json!({"query": "root"}),
            Some(&structured_parts(serde_json::json!({
                "qualified_name": "crate::engine::run"
            }))),
        )
        .unwrap();
    let source = GraphCorrelationToolV1::GetCodeSnippet.public_name();

    assert_eq!(
        lineages.resolve(
            source,
            &serde_json::json!({"qualified_name": "crate::engine::run"})
        ),
        LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::IncapableSelection)
    );
    assert_eq!(
        lineages.resolve(
            source,
            &serde_json::json!({
                "qualified_name": "crate::engine::run",
                "decision_evidence_kind": "provider prose"
            })
        ),
        LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::MalformedSelector)
    );

    let denied_caller = lineages.resolve(
        source,
        &serde_json::json!({
            "qualified_name": "crate::engine::run",
            "decision_evidence_kind": "caller"
        }),
    );
    assert_eq!(
        denied_caller,
        LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::IncapableSelection),
        "a root candidate is not caller evidence before implementation traversal"
    );
    let admitted = lineages.resolve(
        source,
        &serde_json::json!({
            "qualified_name": "crate::engine::run",
            "decision_evidence_kind": "implementation"
        }),
    );
    let LineageAdmissionOutcome::Eligible(admitted) = admitted else {
        panic!("validated implementation purpose must be eligible");
    };
    assert!(admitted.matches_root(&root.root_binding));
    assert_eq!(
        admitted.evidence_purpose(),
        Some(temper_protocol_activity::DecisionEvidenceKindV1::Implementation)
    );
}
