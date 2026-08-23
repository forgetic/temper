use super::*;
use temper_protocol_activity::{DecisionEvidenceKindV1, FocusedTestDiscoveryOutcomeV1};

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
    assert_eq!(
        lineages.resolve(
            GraphCorrelationToolV1::SearchGraph.public_name(),
            &serde_json::json!({"query": "request affinity behavioral regression"}),
        ),
        LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::BroadSelector),
        "an eligible traversal result must not open the semantic fallback",
    );
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

#[test]
fn empty_traversal_opens_one_same_root_semantic_fallback_and_only_its_exact_test() {
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
    lineages
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
    lineages
        .record_with_evidence_kind(
            &correlation(GraphCorrelationTargetKindV1::QualifiedName),
            &serde_json::json!({"qualified_name": "crate::delivery::dispatch"}),
            Some(&structured_parts(serde_json::json!({
                "qualified_name": "crate::delivery::dispatch"
            }))),
            Some(DecisionEvidenceKindV1::Caller),
        )
        .unwrap();

    let traversal_input = serde_json::json!({
        "function_name": "crate::delivery::dispatch",
        "mode": "calls",
        "direction": "inbound",
        "include_tests": true,
    });
    assert!(matches!(
        lineages.resolve(
            GraphCorrelationToolV1::TracePath.public_name(),
            &traversal_input,
        ),
        LineageAdmissionOutcome::Eligible(_)
    ));
    let traversal = lineages
        .record(
            &correlation(GraphCorrelationTargetKindV1::FunctionName),
            &traversal_input,
            Some(&structured_parts(serde_json::json!({
                "function": {"qualified_name": "crate::delivery::dispatch"},
                "callers": []
            }))),
        )
        .unwrap();
    assert_eq!(traversal.root_binding, root.root_binding);
    assert_eq!(
        traversal.focused_test_discovery,
        Some(FocusedTestDiscoveryOutcomeV1::NoEligibleSelector)
    );

    for untrusted in [
        "crate::tests::name_from_task_text",
        "crate::tests::name_from_source",
        "crate::tests::name_from_diagnostic",
    ] {
        assert_eq!(
            lineages.resolve(
                GraphCorrelationToolV1::GetCodeSnippet.public_name(),
                &serde_json::json!({
                    "qualified_name": untrusted,
                    "decision_evidence_kind": "focused_test"
                }),
            ),
            LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::UnknownSelector),
        );
    }

    let query_input = serde_json::json!({"query": "request affinity behavioral regression"});
    let fallback = lineages.resolve(
        GraphCorrelationToolV1::SearchGraph.public_name(),
        &query_input,
    );
    let LineageAdmissionOutcome::Eligible(fallback) = fallback else {
        panic!("settled empty traversal must enable one semantic fallback");
    };
    assert!(fallback.matches_root(&root.root_binding));
    assert_eq!(
        fallback.tool_kind(),
        GraphCorrelationToolV1::SearchGraph
    );

    let fallback_result = lineages
        .record(
            &GraphCorrelationV1::new(
                GraphCorrelationToolV1::SearchGraph,
                GraphCorrelationTargetKindV1::GraphQuery,
                "request affinity behavioral regression",
            )
            .unwrap(),
            &query_input,
            Some(&structured_parts(serde_json::json!({
                "results": [
                    {
                        "qualified_name": "crate::tests::request_affinity_is_stable",
                        "is_test": true
                    },
                    {
                        "qualified_name": "crate::route::unrelated_helper"
                    }
                ]
            }))),
        )
        .unwrap();
    assert_eq!(fallback_result.root_binding, root.root_binding);
    assert_eq!(
        fallback_result.focused_test_discovery,
        Some(FocusedTestDiscoveryOutcomeV1::EligibleSelectorReturned)
    );
    assert_eq!(
        lineages.resolve(
            GraphCorrelationToolV1::SearchGraph.public_name(),
            &serde_json::json!({"query": "another regression search"}),
        ),
        LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::BroadSelector),
        "a completed fallback cannot reopen",
    );

    assert_eq!(
        lineages.resolve(
            GraphCorrelationToolV1::GetCodeSnippet.public_name(),
            &serde_json::json!({
                "qualified_name": "crate::route::unrelated_helper",
                "decision_evidence_kind": "focused_test"
            }),
        ),
        LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::IncapableSelection),
        "a non-test identity returned beside the fallback test remains ineligible",
    );
    let exact = lineages.resolve(
        GraphCorrelationToolV1::GetCodeSnippet.public_name(),
        &serde_json::json!({
            "qualified_name": "crate::tests::request_affinity_is_stable",
            "decision_evidence_kind": "focused_test"
        }),
    );
    let LineageAdmissionOutcome::Eligible(exact) = exact else {
        panic!("fallback-returned exact test must be eligible");
    };
    assert!(exact.matches_root(&root.root_binding));
}
