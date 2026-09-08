use super::*;
use crate::mcp::McpToolCallResult;
use std::collections::BTreeSet;
use temper_protocol_activity::{DecisionEvidenceKindV1, FocusedTestDiscoveryOutcomeV1};

fn record_search(lineages: &mut DecisionAnchorLineages, value: Value) -> String {
    let mut result = McpToolCallResult {
        text: value.to_string(),
        is_error: false,
        typed_parts: Some(structured_parts(value)),
    };
    crate::codebase_memory::provider_output::normalize(&mut result);
    let root = lineages
        .record(
            &correlation(GraphCorrelationTargetKindV1::GraphQuery),
            &serde_json::json!({"query": "focused behavior"}),
            result.typed_parts.as_deref(),
        )
        .unwrap();
    assert_eq!(
        root.focused_test_discovery,
        Some(FocusedTestDiscoveryOutcomeV1::EligibleSelectorReturned)
    );
    root.root_binding
}

fn focused_references(lineages: &mut DecisionAnchorLineages, root: &str) -> Vec<String> {
    let guidance = lineages.recovery_selector_guidance(root).unwrap();
    guidance
        .split("focused_test_result")
        .skip(1)
        .map(|part| {
            part.split_once('=')
                .unwrap()
                .1
                .split([',', '.', ' ', ']'])
                .next()
                .unwrap()
                .to_string()
        })
        .collect()
}

fn expand_focused_reference(
    lineages: &mut DecisionAnchorLineages,
    root: &str,
    reference: &str,
) -> Value {
    let mut input = serde_json::json!({
        "qualified_name": reference,
        "decision_evidence_kind": "focused_test"
    });
    let admission = lineages.resolve_for_active_root(
        GraphCorrelationToolV1::GetCodeSnippet.public_name(),
        &input,
        Some(root),
    );
    let LineageAdmissionOutcome::Eligible(admission) = admission else {
        panic!("the classified test reference must retain exact root admission");
    };
    assert!(admission.matches_root(root));
    assert_eq!(
        admission.evidence_purpose(),
        Some(DecisionEvidenceKindV1::FocusedTest)
    );
    lineages
        .expand_recovery_selector(
            GraphCorrelationToolV1::GetCodeSnippet.public_name(),
            &mut input,
            Some(DecisionEvidenceKindV1::FocusedTest),
        )
        .unwrap()
        .unwrap();
    input
}

#[test]
fn release_table_expands_distinct_focused_tests_with_exact_dotted_provider_names() {
    let provider_names = [
        "temper-v1-demo.tests.parser.reports_missing",
        "temper-v1-demo.tests.evaluate.reports_missing",
    ];
    let mut lineages = DecisionAnchorLineages::default();
    let root = record_search(
        &mut lineages,
        serde_json::json!({
            "total": 2, "search_mode": "bm25",
            "cols": ["qn", "label", "file", "lines", "rank"],
            "rows": [
                [provider_names[0], "Function", "tests/parser.rs", "4-9", -13.6],
                [provider_names[1], "Function", "tests/evaluate.rs", "4-8", -13.4]
            ],
            "has_more": false
        }),
    );
    let references = focused_references(&mut lineages, &root);
    assert_eq!(
        references.len(),
        2,
        "equal terminal names remain distinct sources"
    );
    let expanded = references
        .iter()
        .map(|reference| {
            expand_focused_reference(&mut lineages, &root, reference)["qualified_name"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        expanded,
        provider_names.map(str::to_string).into_iter().collect()
    );
}

#[test]
fn focused_provider_spelling_preserves_canonical_admission_and_test_classification() {
    for field in ["qualified_name", "qualifiedName"] {
        for provider_name in ["demo.tests.keeps_affinity", "demo::tests::keeps_affinity"] {
            let mut record = serde_json::json!({"is_test": true});
            record[field] = serde_json::json!(provider_name);
            let mut lineages = DecisionAnchorLineages::default();
            let root = record_search(
                &mut lineages,
                serde_json::json!({"results": [
                    record,
                    {"qualified_name": "demo.src.keeps_affinity", "is_test": false}
                ]}),
            );
            let canonical = serde_json::json!({
                "qualified_name": "demo::tests::keeps_affinity",
                "decision_evidence_kind": "focused_test"
            });
            assert!(matches!(
                lineages.resolve_for_active_root(
                    GraphCorrelationToolV1::GetCodeSnippet.public_name(),
                    &canonical,
                    Some(&root)
                ),
                LineageAdmissionOutcome::Eligible(_)
            ));
            let references = focused_references(&mut lineages, &root);
            assert_eq!(references.len(), 1);
            let expanded = expand_focused_reference(&mut lineages, &root, &references[0]);
            assert_eq!(expanded["qualified_name"], provider_name);
            let unclassified = serde_json::json!({
                "qualified_name": "demo::src::keeps_affinity",
                "decision_evidence_kind": "focused_test"
            });
            assert_eq!(
                lineages.resolve_for_active_root(
                    GraphCorrelationToolV1::GetCodeSnippet.public_name(),
                    &unclassified,
                    Some(&root)
                ),
                LineageAdmissionOutcome::Ineligible(LineageAdmissionStatus::IncapableSelection)
            );
        }
    }
}
