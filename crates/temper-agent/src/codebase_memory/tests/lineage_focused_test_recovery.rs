use super::*;
use temper_protocol_activity::DecisionEvidenceKindV1;

#[test]
fn caller_to_test_traversal_requires_typed_origins_and_unlocks_exact_test_source() {
    let mut lineages = DecisionAnchorLineages::default();
    let root = lineages
        .record(
            &correlation(GraphCorrelationTargetKindV1::Pattern),
            &serde_json::json!({"pattern": "select_worker"}),
            Some(&structured_parts(serde_json::json!({
                "qualified_name": "crate::route::select_worker"
            }))),
        )
        .unwrap();
    let implementation = lineages
        .record_with_evidence_kind(
            &correlation(GraphCorrelationTargetKindV1::QualifiedName),
            &serde_json::json!({"qualified_name": "crate::route::select_worker"}),
            Some(&structured_parts(serde_json::json!({
                "qualified_name": "crate::route::select_worker",
                "callers": [{"qualified_name": "crate::delivery::dispatch"}]
            }))),
            Some(DecisionEvidenceKindV1::Implementation),
        )
        .unwrap();
    assert_eq!(implementation.root_binding, root.root_binding);
    let caller = lineages
        .record_with_evidence_kind(
            &correlation(GraphCorrelationTargetKindV1::QualifiedName),
            &serde_json::json!({"qualified_name": "crate::delivery::dispatch"}),
            Some(&structured_parts(serde_json::json!({
                "qualified_name": "crate::delivery::dispatch",
                "callees": [{"qualified_name": "crate::route::select_worker"}]
            }))),
            Some(DecisionEvidenceKindV1::Caller),
        )
        .unwrap();
    assert_eq!(caller.root_binding, root.root_binding);

    assert_eq!(
        lineages.resolve(
            GraphCorrelationToolV1::GetCodeSnippet.public_name(),
            &serde_json::json!({
                "qualified_name": "crate::route::select_worker",
                "decision_evidence_kind": "focused_test"
            }),
        ),
        LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::IncapableSelection),
        "an implementation identity is not a focused-test selector",
    );
    let traversal_input = serde_json::json!({
        "function_name": "crate::delivery::dispatch",
        "mode": "calls",
        "direction": "inbound",
        "include_tests": true,
    });
    let traversal = lineages.resolve(
        GraphCorrelationToolV1::TracePath.public_name(),
        &traversal_input,
    );
    let LineageAdmissionOutcome::Eligible(traversal) = traversal else {
        panic!("typed caller evidence must admit a test-inclusive traversal");
    };
    assert!(traversal.matches_root(&root.root_binding));
    assert_eq!(
        traversal.recovery_purpose(),
        Some(DecisionEvidenceKindV1::FocusedTest)
    );

    lineages
        .record_with_evidence_kind(
            &correlation(GraphCorrelationTargetKindV1::FunctionName),
            &traversal_input,
            Some(&structured_parts(serde_json::json!({
                "function": {"qualified_name": "crate::delivery::dispatch"},
                "callers": [{
                    "qualified_name": "crate::tests::keeps_affinity",
                    "is_test": true
                }]
            }))),
            None,
        )
        .unwrap();
    let exact_test = lineages.resolve(
        GraphCorrelationToolV1::GetCodeSnippet.public_name(),
        &serde_json::json!({
            "qualified_name": "crate::tests::keeps_affinity",
            "decision_evidence_kind": "focused_test"
        }),
    );
    let LineageAdmissionOutcome::Eligible(exact_test) = exact_test else {
        panic!("exact traversal-returned test must become eligible");
    };
    assert!(exact_test.matches_root(&root.root_binding));
}
