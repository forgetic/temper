use super::*;
use temper_protocol_activity::{
    CallerDiscoveryOutcomeV1, DecisionEvidenceKindV1, FocusedTestDiscoveryOutcomeV1,
};

#[test]
fn caller_to_test_traversal_requires_typed_origins_and_unlocks_exact_test_source() {
    let mut lineages = DecisionAnchorLineages::default();
    let root = lineages
        .record(
            &correlation(GraphCorrelationTargetKindV1::Pattern),
            &serde_json::json!({"pattern": "select_worker"}),
            Some(&structured_parts(serde_json::json!({
                "results": [
                    {"qualified_name": "crate::route::select_worker"},
                    {"qualified_name": "crate::delivery::dispatch"},
                    {
                        "qualified_name": "crate::tests::keeps_affinity",
                        "is_test": true
                    }
                ]
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
    assert_eq!(
        lineages.resolve(
            GraphCorrelationToolV1::GetCodeSnippet.public_name(),
            &serde_json::json!({
                "qualified_name": "crate::delivery::dispatch",
                "decision_evidence_kind": "caller"
            }),
        ),
        LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::IncapableSelection),
        "an implementation result's caller-shaped candidate must await traversal",
    );
    let caller_trace_input = serde_json::json!({
        "function_name": "crate::route::select_worker",
        "mode": "calls",
        "direction": "inbound",
    });
    assert!(matches!(
        lineages.resolve(
            GraphCorrelationToolV1::TracePath.public_name(),
            &caller_trace_input,
        ),
        LineageAdmissionOutcome::Eligible(_)
    ));
    let caller_trace = lineages
        .record(
            &correlation(GraphCorrelationTargetKindV1::FunctionName),
            &caller_trace_input,
            Some(&structured_parts(serde_json::json!({
                "function": {"qualified_name": "crate::route::select_worker"},
                "callers": [{"qualified_name": "crate::delivery::dispatch"}]
            }))),
        )
        .unwrap();
    assert_eq!(
        caller_trace.caller_discovery,
        Some(CallerDiscoveryOutcomeV1::EligibleSelectorReturned)
    );
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
    assert_eq!(
        lineages.resolve(
            GraphCorrelationToolV1::GetCodeSnippet.public_name(),
            &serde_json::json!({
                "qualified_name": "crate::tests::keeps_affinity",
                "decision_evidence_kind": "focused_test"
            }),
        ),
        LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::IncapableSelection),
        "an over-returned test-shaped candidate must await a typed test route",
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
    let caller_trace_input = serde_json::json!({
        "function_name": "crate::route::select_worker",
        "direction": "inbound",
    });
    assert!(matches!(
        lineages.resolve(
            GraphCorrelationToolV1::TracePath.public_name(),
            &caller_trace_input,
        ),
        LineageAdmissionOutcome::Eligible(_)
    ));
    lineages
        .record(
            &correlation(GraphCorrelationTargetKindV1::FunctionName),
            &caller_trace_input,
            Some(&structured_parts(serde_json::json!({
                "function": {"qualified_name": "crate::route::select_worker"},
                "callers": [{"qualified_name": "crate::delivery::dispatch"}]
            }))),
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

    let foreign_root = lineages
        .record(
            &GraphCorrelationV1::new(
                GraphCorrelationToolV1::SearchGraph,
                GraphCorrelationTargetKindV1::GraphQuery,
                "independent behavior root",
            )
            .unwrap(),
            &serde_json::json!({"query": "independent behavior root"}),
            Some(&structured_parts(serde_json::json!({
                "results": [{
                    "qualified_name": "temper-v1-hash.tests.cross_root.same_test",
                    "name": "same_test",
                    "label": "Function"
                }]
            }))),
        )
        .unwrap();
    assert_ne!(foreign_root.root_binding, root.root_binding);

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
    assert_eq!(
        lineages.resolve(
            GraphCorrelationToolV1::SearchGraph.public_name(),
            &serde_json::json!({"query": "duplicate behavioral fallback"}),
        ),
        LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::BroadSelector),
        "one pending fallback closes the same-root fallback route",
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
                        "name": "request_affinity_is_stable",
                        "qualified_name": "temper-v1-hash.tests.request_affinity.request_affinity_is_stable",
                        "label": "Function",
                        "file_path": "tests/request_affinity.rs",
                        "degree": 1
                    },
                    {
                        "functionName": "request_affinity_survives_retry",
                        "qualifiedName": "temper-v1-hash.tests.request_affinity.request_affinity_survives_retry",
                        "label": "Function"
                    },
                    {
                        "name": "request_affinity_helper",
                        "qualified_name": "temper-v1-hash.src.request_affinity_helper",
                        "label": "Function"
                    },
                    {
                        "qualified_name": "temper-v1-hash.tests.first.shared_selector",
                        "label": "Function"
                    },
                    {
                        "qualified_name": "temper-v1-hash.tests.second.shared_selector",
                        "label": "Function"
                    },
                    {
                        "name": "same_test",
                        "qualified_name": "temper-v1-hash.tests.cross_root.same_test",
                        "label": "Function"
                    }
                ],
                "total": 6,
                "has_more": false
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

    for (selector, status) in [
        (
            "temper-v1-hash.tests.cross_root.same_test",
            LineageAdmissionStatus::AmbiguousSelector,
        ),
        (
            "shared_selector",
            LineageAdmissionStatus::IncapableSelection,
        ),
        (
            "tests::request_affinity::request_affinity_is_stable",
            LineageAdmissionStatus::UnknownSelector,
        ),
        (
            "temper-v1-hash.tests.invented_test",
            LineageAdmissionStatus::UnknownSelector,
        ),
    ] {
        assert_eq!(
            lineages.resolve(
                GraphCorrelationToolV1::GetCodeSnippet.public_name(),
                &serde_json::json!({
                    "qualified_name": selector,
                    "decision_evidence_kind": "focused_test"
                }),
            ),
            LineageAdmissionOutcome::Ineligible(status),
        );
    }

    let non_test_input = serde_json::json!({
        "qualified_name": "temper-v1-hash.src.request_affinity_helper",
        "decision_evidence_kind": "focused_test"
    });
    assert!(matches!(
        lineages.resolve(
            GraphCorrelationToolV1::GetCodeSnippet.public_name(),
            &non_test_input,
        ),
        LineageAdmissionOutcome::Eligible(_)
    ));
    let non_test = lineages
        .record_with_evidence_kind(
            &correlation(GraphCorrelationTargetKindV1::QualifiedName),
            &non_test_input,
            Some(&structured_parts(serde_json::json!({
                "qualified_name": "temper-v1-hash.src.request_affinity_helper",
                "name": "request_affinity_helper",
                "source": "helper source",
                "is_test": false
            }))),
            Some(DecisionEvidenceKindV1::FocusedTest),
        )
        .unwrap();
    assert_eq!(non_test.root_binding, root.root_binding);
    assert_eq!(non_test.decision_evidence_kind, None);

    let exact_input = serde_json::json!({
        "qualified_name": "temper-v1-hash.tests.request_affinity.request_affinity_is_stable",
        "decision_evidence_kind": "focused_test"
    });
    let exact = lineages.resolve(
        GraphCorrelationToolV1::GetCodeSnippet.public_name(),
        &exact_input,
    );
    let LineageAdmissionOutcome::Eligible(exact) = exact else {
        panic!("fallback-returned exact provider identity must be eligible");
    };
    assert!(exact.matches_root(&root.root_binding));

    let test_source = lineages
        .record_with_evidence_kind(
            &correlation(GraphCorrelationTargetKindV1::QualifiedName),
            &exact_input,
            Some(&structured_parts(serde_json::json!({
                "qualified_name": "temper-v1-hash.tests.request_affinity.request_affinity_is_stable",
                "name": "request_affinity_is_stable",
                "source": "typed test source",
                "is_test": true
            }))),
            Some(DecisionEvidenceKindV1::FocusedTest),
        )
        .unwrap();
    assert_eq!(test_source.root_binding, root.root_binding);
    assert_eq!(
        test_source.decision_evidence_kind,
        Some(DecisionEvidenceKindV1::FocusedTest)
    );
    assert!(test_source.canonical_target_digests.contains(
        &GraphCorrelationV1::target_digest(
            "tests::request_affinity::request_affinity_is_stable"
        )
        .unwrap()
    ));
}
