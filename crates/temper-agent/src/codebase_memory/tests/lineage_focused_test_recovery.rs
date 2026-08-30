use super::*;
use temper_protocol_activity::{
    CallerDiscoveryOutcomeV1, DecisionEvidenceKindV1, FocusedTestDiscoveryOutcomeV1,
};

#[test]
fn provider_classified_focused_test_root_unlocks_only_its_exact_source() {
    let mut lineages = DecisionAnchorLineages::default();
    let root = lineages
        .record(
            &GraphCorrelationV1::new(
                GraphCorrelationToolV1::SearchGraph,
                GraphCorrelationTargetKindV1::GraphQuery,
                "behavioral regression",
            )
            .unwrap(),
            &serde_json::json!({"query": "behavioral regression"}),
            Some(&structured_parts(serde_json::json!({
                "results": [
                    {"qualified_name": "crate::route::select_worker"},
                    {
                        "qualified_name": "crate::tests::keeps_affinity",
                        "is_test": true
                    }
                ]
            }))),
        )
        .unwrap();
    assert_eq!(
        root.focused_test_discovery,
        Some(FocusedTestDiscoveryOutcomeV1::EligibleSelectorReturned)
    );

    assert_eq!(
        lineages.resolve(
            GraphCorrelationToolV1::GetCodeSnippet.public_name(),
            &serde_json::json!({
                "qualified_name": "crate::route::select_worker",
                "decision_evidence_kind": "focused_test"
            }),
        ),
        LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::IncapableSelection),
        "an unclassified implementation candidate cannot become focused-test evidence",
    );
    let exact = lineages.resolve_for_active_root(
        GraphCorrelationToolV1::GetCodeSnippet.public_name(),
        &serde_json::json!({
            "qualified_name": "crate::tests::keeps_affinity",
            "decision_evidence_kind": "focused_test"
        }),
        Some(&root.root_binding),
    );
    let LineageAdmissionOutcome::Eligible(exact) = exact else {
        panic!("the exact provider-classified test must be admitted");
    };
    assert!(exact.matches_root(&root.root_binding));
    assert_eq!(
        exact.evidence_purpose(),
        Some(DecisionEvidenceKindV1::FocusedTest)
    );
}

#[test]
fn active_root_rejects_sibling_task_and_failed_selector_substitutions() {
    let mut lineages = DecisionAnchorLineages::default();
    let implementation = lineages
        .record(
            &correlation(GraphCorrelationTargetKindV1::GraphQuery),
            &serde_json::json!({"query": "implementation behavior"}),
            Some(&structured_parts(serde_json::json!({
                "qualified_name": "crate::route::select_worker"
            }))),
        )
        .unwrap();
    let focused = lineages
        .record(
            &GraphCorrelationV1::new(
                GraphCorrelationToolV1::SearchGraph,
                GraphCorrelationTargetKindV1::GraphQuery,
                "focused behavior",
            )
            .unwrap(),
            &serde_json::json!({"query": "focused behavior"}),
            Some(&structured_parts(serde_json::json!({
                "qualified_name": "crate::tests::keeps_affinity",
                "is_test": true
            }))),
        )
        .unwrap();
    assert_ne!(implementation.root_binding, focused.root_binding);

    assert_eq!(
        lineages.resolve_for_active_root(
            GraphCorrelationToolV1::GetCodeSnippet.public_name(),
            &serde_json::json!({
                "qualified_name": "crate::tests::keeps_affinity",
                "decision_evidence_kind": "focused_test"
            }),
            Some(&implementation.root_binding),
        ),
        LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::UnknownSelector),
    );
    assert_eq!(
        lineages.resolve_for_active_root(
            GraphCorrelationToolV1::SearchGraph.public_name(),
            &serde_json::json!({"query": "test name copied from task text"}),
            Some(&implementation.root_binding),
        ),
        LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::BroadSelector),
        "task-derived recovery queries never attach to an established root",
    );
    assert_eq!(
        lineages.resolve_for_active_root(
            GraphCorrelationToolV1::GetCodeSnippet.public_name(),
            &serde_json::json!({
                "qualified_name": "never-returned-test",
                "decision_evidence_kind": "focused_test"
            }),
            Some(&focused.root_binding),
        ),
        LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::MalformedSelector),
    );
}

#[test]
fn implementation_caller_route_remains_root_local_and_later_turn_ordered() {
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
                "qualified_name": "crate::route::select_worker"
            }))),
            Some(DecisionEvidenceKindV1::Implementation),
        )
        .unwrap();
    let trace_input = serde_json::json!({
        "function_name": "select_worker",
        "mode": "calls",
        "direction": "inbound"
    });
    assert!(matches!(
        lineages.resolve_for_active_root(
            GraphCorrelationToolV1::TracePath.public_name(),
            &trace_input,
            Some(&root.root_binding),
        ),
        LineageAdmissionOutcome::Eligible(_)
    ));
    let trace = lineages
        .record(
            &correlation(GraphCorrelationTargetKindV1::FunctionName),
            &trace_input,
            Some(&structured_parts(serde_json::json!({
                "function": {"qualified_name": "crate::route::select_worker"},
                "callers": [{"qualified_name": "crate::delivery::dispatch"}]
            }))),
        )
        .unwrap();
    assert_eq!(
        trace.caller_discovery,
        Some(CallerDiscoveryOutcomeV1::EligibleSelectorReturned)
    );
    let caller = lineages.resolve_for_active_root(
        GraphCorrelationToolV1::GetCodeSnippet.public_name(),
        &serde_json::json!({
            "qualified_name": "crate::delivery::dispatch",
            "decision_evidence_kind": "caller"
        }),
        Some(&root.root_binding),
    );
    let LineageAdmissionOutcome::Eligible(caller) = caller else {
        panic!("the traversal-returned caller must be admitted on the same root");
    };
    assert!(caller.matches_root(&root.root_binding));
}
