//! Exercise source sanitization before the real lineage decoder.

use super::*;
use crate::codebase_memory::lineage::DecisionAnchorLineages;
use serde_json::json;
use temper_protocol_activity::{
    DecisionAnchorLineageStageV1, DecisionAnchorLineageV1, FocusedTestDiscoveryOutcomeV1,
    GraphCorrelationTargetKindV1, GraphCorrelationToolV1, GraphCorrelationV1,
};

const LOCAL: &str = "def sentinel():\n    return 'checkout-A'\n";
const FOREIGN: &str = "def sentinel():\n    return 'checkout-B'\n";

fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("sentinel.py"), LOCAL).unwrap();
    root
}

fn response(value: Value) -> McpToolCallResult {
    McpToolCallResult {
        text: value.to_string(),
        is_error: false,
        typed_parts: Some(vec![
            McpToolResultPart::Content(json!({"type":"text","text":value.to_string()})),
            McpToolResultPart::StructuredContent(value),
        ]),
    }
}

fn record(
    lineages: &mut DecisionAnchorLineages,
    kind: GraphCorrelationTargetKindV1,
    input: Value,
    result: &McpToolCallResult,
) -> DecisionAnchorLineageV1 {
    let tool = match kind {
        GraphCorrelationTargetKindV1::GraphQuery => GraphCorrelationToolV1::SearchGraph,
        GraphCorrelationTargetKindV1::Pattern => GraphCorrelationToolV1::SearchCode,
        GraphCorrelationTargetKindV1::QualifiedName => GraphCorrelationToolV1::GetCodeSnippet,
        _ => unreachable!(),
    };
    let correlation = GraphCorrelationV1::new(tool, kind, "test selector").unwrap();
    lineages
        .record(&correlation, &input, result.typed_parts.as_deref())
        .unwrap()
}

#[test]
fn mapped_nested_identities_survive_source_guard_and_keep_one_lineage_root() {
    let root = fixture();
    let mut lineages = DecisionAnchorLineages::default();
    let mut discovery = response(json!({"results":[{
        "results":[{"name":"sentinel","qualifiedName":"fixture.sentinel",
            "file_path":"sentinel.py","is_test":false},
            {"name":"focused_sentinel","qualifiedName":"fixture.focused_sentinel",
                "file_path":"tests/sentinel.py","is_test":true}],
        "callers":[{"qualifiedName":"fixture.caller"}]
    }]}));
    assert!(verify_at_root(root.path(), &mut discovery));
    let anchor = record(
        &mut lineages,
        GraphCorrelationTargetKindV1::GraphQuery,
        json!({"query":"sentinel routing"}),
        &discovery,
    );
    assert_eq!(anchor.result_target_kinds.len(), 3);
    assert_eq!(
        anchor.focused_test_discovery,
        Some(FocusedTestDiscoveryOutcomeV1::EligibleSelectorReturned)
    );
    assert!(discovery.text.contains("qualifiedName"));

    let mut refinement = response(json!({"results":[{
        "name":"sentinel","qualified_name":"fixture.sentinel","file_path":"sentinel.py",
        "related_source_references":[{"qualifiedName":"fixture.caller"}]
    }]}));
    assert!(verify_at_root(root.path(), &mut refinement));
    let refined = record(
        &mut lineages,
        GraphCorrelationTargetKindV1::Pattern,
        json!({"pattern":"sentinel"}),
        &refinement,
    );
    assert_eq!(refined.stage, DecisionAnchorLineageStageV1::CarryForward);
    assert_eq!(refined.root_binding, anchor.root_binding);

    let mut caller = response(json!({"qualified_name":"fixture.caller",
    "file_path":"sentinel.py","source":LOCAL,"source_metadata":{
        "related_source_references":[{"qualifiedName":"fixture.test_sentinel"}]
    }}));
    assert!(verify_at_root(root.path(), &mut caller));
    let caller = record(
        &mut lineages,
        GraphCorrelationTargetKindV1::QualifiedName,
        json!({"qualified_name":"fixture.caller"}),
        &caller,
    );
    assert_eq!(caller.root_binding, anchor.root_binding);

    let mut related = response(json!({"qualified_name":"fixture.test_sentinel",
        "file_path":"sentinel.py","source":LOCAL}));
    assert!(verify_at_root(root.path(), &mut related));
    let related = record(
        &mut lineages,
        GraphCorrelationTargetKindV1::QualifiedName,
        json!({"qualified_name":"fixture.test_sentinel"}),
        &related,
    );
    assert_eq!(related.stage, DecisionAnchorLineageStageV1::CarryForward);
    assert_eq!(related.root_binding, anchor.root_binding);
    // Preserving a selector does not grant a source purpose or mutation authority.
    assert!(related.decision_evidence_kind.is_none());
}

#[test]
fn retained_identity_containers_cannot_hide_foreign_source_or_raw_sidecars() {
    let root = fixture();
    for field in [
        "source_metadata",
        "sourceMetadata",
        "related_source_references",
        "callers",
    ] {
        for source in [LOCAL, FOREIGN] {
            let record = json!({"qualifiedName":"fixture.sentinel","file_path":"sentinel.py",
                "source":source,"text":FOREIGN,"extra":{"prompt":FOREIGN}});
            let nested = if field.starts_with("source") {
                record
            } else {
                json!([record])
            };
            let mut value = json!({"results":[]});
            value[field] = nested;
            let mut result = response(value);
            assert_eq!(
                verify_at_root(root.path(), &mut result),
                source == LOCAL,
                "{field}"
            );
            if source == LOCAL {
                assert!(!result.text.contains("checkout-B"));
                assert!(result.text.contains("qualifiedName"));
            }
        }
    }
}

#[test]
fn source_path_aliases_are_verified_together_and_identity_conflicts_stay_ineligible() {
    let root = fixture();
    for alias in ["filePath", "source_path", "sourcePath"] {
        let mut value = json!({"qualifiedName":"fixture.sentinel","source":LOCAL});
        value[alias] = json!("sentinel.py");
        assert!(verify_at_root(root.path(), &mut response(value.clone())));
        value["file_path"] = json!("other.py");
        assert!(!verify_at_root(root.path(), &mut response(value)));
    }
    let mut result = response(json!({"results":[{"qualified_name":"fixture.sentinel",
        "qualifiedName":"other.sentinel","file_path":"sentinel.py"}]}));
    assert!(verify_at_root(root.path(), &mut result));
    let lineage = record(
        &mut DecisionAnchorLineages::default(),
        GraphCorrelationTargetKindV1::GraphQuery,
        json!({"query":"sentinel"}),
        &result,
    );
    assert!(lineage.result_target_kinds.is_empty());
}

#[test]
fn malformed_new_metadata_containers_do_not_reach_presentation() {
    let root = fixture();
    for (field, malformed) in [
        ("source_metadata", json!(FOREIGN)),
        (
            "related_source_references",
            json!({"qualifiedName":"fixture.sentinel"}),
        ),
        ("callers", json!([true])),
        ("callers", json!({"results":[],"message":FOREIGN})),
        ("function", json!(["fixture.sentinel"])),
    ] {
        let mut value = json!({"results":[]});
        value[field] = malformed;
        assert!(
            !verify_at_root(root.path(), &mut response(value)),
            "{field}"
        );
    }
}

#[test]
fn normalized_reference_tables_remain_verifiable_in_search_and_source_results() {
    let root = fixture();
    for source in [LOCAL, FOREIGN] {
        let mut result = response(json!({"results":[{
            "qualified_name":"fixture.sentinel","file_path":"sentinel.py",
            "source_metadata":{"callers":{
                "cols":["name","source"],"groups":[{
                    "qn_prefix":"fixture","file":"sentinel.py","rows":[["caller",source]]
                }]
            }}
        }]}));
        crate::codebase_memory::provider_output::normalize(&mut result);
        assert!(result.typed_parts.is_some());
        assert_eq!(verify_at_root(root.path(), &mut result), source == LOCAL);
        if source == LOCAL {
            let value: Value = serde_json::from_str(&result.text).unwrap();
            assert_eq!(
                value["results"][0]["source_metadata"]["callers"]["results"][0]["qualified_name"],
                "fixture.caller"
            );
        }
    }
}
