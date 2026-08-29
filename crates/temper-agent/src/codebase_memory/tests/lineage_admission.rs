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
    for (input, expected) in [
        (
            serde_json::json!({}),
            LineageAdmissionStatus::MalformedSelector,
        ),
        (
            serde_json::json!({"function_name": ""}),
            LineageAdmissionStatus::MalformedSelector,
        ),
        (
            serde_json::json!({"qualified_name": PRIVATE_SELECTOR}),
            LineageAdmissionStatus::IncapableSelection,
        ),
        (
            serde_json::json!({"function_name": "not_returned"}),
            LineageAdmissionStatus::UnknownSelector,
        ),
    ] {
        assert_eq!(
            lineages.resolve(GraphCorrelationToolV1::TracePath.public_name(), &input),
            LineageAdmissionOutcome::Ineligible(expected),
        );
    }
    let admission = lineages.resolve(
        GraphCorrelationToolV1::TracePath.public_name(),
        &serde_json::json!({"function_name": "run"}),
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
fn opaque_recovery_reference_resolves_and_expands_without_exposing_selector() {
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

    let guidance = lineages
        .recovery_selector_guidance(&root.root_binding)
        .expect("implementation result exposes an opaque trace selector reference");
    assert!(!guidance.contains(PRIVATE_SELECTOR));
    assert!(!guidance.contains(&root.root_binding));
    assert!(
        guidance.len()
            <= crate::codebase_memory::result_presentation::RECOVERY_SELECTOR_GUIDANCE_RESERVE_BYTES
    );
    let reference = guidance
        .split_once("implementation_evidence_result=")
        .and_then(|(_, rest)| rest.split([',', '.']).next())
        .expect("closed implementation trace reference");
    assert!(reference.starts_with("temper-recovery-selector:"));

    let mut trace_input = serde_json::json!({"function_name": reference});
    let admission = lineages.resolve(
        GraphCorrelationToolV1::TracePath.public_name(),
        &trace_input,
    );
    let LineageAdmissionOutcome::Eligible(admission) = admission else {
        panic!("opaque reference must resolve to its provider-derived selector");
    };
    assert!(admission.matches_root(&root.root_binding));
    lineages
        .expand_recovery_selector(
            GraphCorrelationToolV1::TracePath.public_name(),
            &mut trace_input,
            None,
        )
        .unwrap();
    assert_eq!(trace_input["function_name"], PRIVATE_SELECTOR);

    let unknown_reference = format!("{reference}-inexact");
    let mut unknown_input = serde_json::json!({"function_name": unknown_reference});
    assert_eq!(
        lineages.resolve(
            GraphCorrelationToolV1::TracePath.public_name(),
            &unknown_input,
        ),
        LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::MalformedSelector),
    );
    assert!(
        lineages
            .expand_recovery_selector(
                GraphCorrelationToolV1::TracePath.public_name(),
                &mut unknown_input,
                None,
            )
            .is_err()
    );

    let mut wrong_purpose = serde_json::json!({"qualified_name": reference});
    assert!(
        lineages
            .expand_recovery_selector(
                GraphCorrelationToolV1::GetCodeSnippet.public_name(),
                &mut wrong_purpose,
                Some(temper_protocol_activity::DecisionEvidenceKindV1::Caller),
            )
            .is_err(),
        "a trace reference cannot be substituted for a caller-source reference",
    );
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
