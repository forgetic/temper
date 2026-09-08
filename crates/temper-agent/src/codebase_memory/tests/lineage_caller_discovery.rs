use super::*;
use crate::mcp::McpToolCallResult;
use serde_json::json;
use temper_protocol_activity::{
    CallerDiscoveryOutcomeV1, DecisionAnchorLineageV1, DecisionEvidenceKindV1,
};

const IMPLEMENTATION: &str = "demo.src.parser.parse_policy";

fn implementation_lineage(commit: bool) -> (DecisionAnchorLineages, String) {
    let mut lineages = DecisionAnchorLineages::default();
    let source = structured_parts(json!({"qualified_name": IMPLEMENTATION}));
    let root = lineages
        .record(
            &correlation(GraphCorrelationTargetKindV1::GraphQuery),
            &json!({"query": "policy parsing"}),
            Some(&source),
        )
        .unwrap();
    if commit {
        let implementation = lineages
            .record_with_evidence_kind(
                &correlation(GraphCorrelationTargetKindV1::QualifiedName),
                &json!({"qualified_name": IMPLEMENTATION}),
                Some(&source),
                Some(DecisionEvidenceKindV1::Implementation),
            )
            .unwrap();
        assert_eq!(
            implementation.decision_evidence_kind,
            Some(DecisionEvidenceKindV1::Implementation)
        );
    }
    (lineages, root.root_binding)
}

fn trace_input() -> Value {
    json!({"function_name": "parse_policy", "direction": "inbound", "mode": "calls", "include_tests": false})
}

fn empty_release_trace() -> Value {
    json!({"function": IMPLEMENTATION, "direction": "inbound", "callers_total": 0,
        "callers": {"cols": ["name", "hop"], "groups": []}})
}

fn normalized(value: Value) -> McpToolCallResult {
    let mut result = McpToolCallResult {
        text: value.to_string(),
        is_error: false,
        typed_parts: Some(structured_parts(value)),
    };
    crate::codebase_memory::provider_output::normalize(&mut result);
    result
}

fn record_trace(
    lineages: &mut DecisionAnchorLineages,
    input: &Value,
    value: Value,
) -> Option<DecisionAnchorLineageV1> {
    let result = normalized(value);
    lineages.record(
        &correlation(GraphCorrelationTargetKindV1::FunctionName),
        input,
        result.typed_parts.as_deref(),
    )
}

#[test]
fn release_empty_caller_table_reports_zero_only_after_implementation_source() {
    for commit in [false, true] {
        let (mut lineages, root) = implementation_lineage(commit);
        let trace = record_trace(&mut lineages, &trace_input(), empty_release_trace()).unwrap();
        assert_eq!(trace.root_binding, root);
        assert_eq!(trace.stage, DecisionAnchorLineageStageV1::CarryForward);
        assert_eq!(
            trace.caller_discovery,
            Some(if commit {
                CallerDiscoveryOutcomeV1::NoProductionCallersReported
            } else {
                CallerDiscoveryOutcomeV1::NoEligibleSelector
            })
        );
        assert_eq!(trace.decision_evidence_kind, None);
    }
}

#[test]
fn opaque_trace_preserves_qualified_source_in_provider_echoed_zero_report() {
    let (mut lineages, root) = implementation_lineage(true);
    let guidance = lineages.recovery_selector_guidance(&root).unwrap();
    let reference = guidance
        .split_once("implementation_evidence_result=")
        .and_then(|(_, rest)| rest.split([',', '.']).next())
        .unwrap();
    let mut input = trace_input();
    input["function_name"] = json!(reference);
    lineages
        .reserve_implementation_trace_reference(&input, Some(&root))
        .unwrap()
        .unwrap();
    lineages
        .expand_recovery_selector(GraphCorrelationToolV1::TracePath.public_name(), &mut input, None)
        .unwrap()
        .unwrap();
    assert_eq!(input["function_name"], IMPLEMENTATION);
    // The real provider echoes the request selector, including short names.
    let result = normalized(json!({
        "function": input["function_name"], "direction": "inbound", "mode": "calls",
        "callers_total": 0, "callers": {"cols": ["name", "hop"], "groups": []}
    }));
    let trace = lineages.record(
        &correlation(GraphCorrelationTargetKindV1::FunctionName), &input,
        result.typed_parts.as_deref(),
    ).unwrap();
    assert_eq!(trace.root_binding, root);
    assert_eq!(trace.caller_discovery, Some(CallerDiscoveryOutcomeV1::NoProductionCallersReported));
}

#[test]
fn release_populated_caller_table_preserves_exact_caller_source_admission() {
    let (mut lineages, root) = implementation_lineage(true);
    let trace = record_trace(
        &mut lineages,
        &trace_input(),
        json!({
            "function": IMPLEMENTATION, "direction": "inbound", "callers_total": 1,
            "callers": {"cols": ["name", "hop"], "groups": [
                {"qn_prefix": "demo.src.delivery", "rows": [["dispatch", 1]]}
            ]}
        }),
    )
    .unwrap();
    assert_eq!(trace.root_binding, root);
    assert_eq!(
        trace.caller_discovery,
        Some(CallerDiscoveryOutcomeV1::EligibleSelectorReturned)
    );
    assert!(matches!(lineages.resolve_for_active_root(
        GraphCorrelationToolV1::GetCodeSnippet.public_name(),
        &json!({"qualified_name": "demo.src.delivery.dispatch", "decision_evidence_kind": "caller"}),
        Some(&root),
    ), LineageAdmissionOutcome::Eligible(_)));
}

#[test]
fn complete_flat_report_accepts_explicit_call_edges_and_selected_qualified_input() {
    let (mut lineages, root) = implementation_lineage(true);
    let input = json!({"function_name": IMPLEMENTATION, "direction": "inbound",
        "mode": "calls", "include_tests": false, "edge_types": ["CALLS"], "depth": 1,
        "limit": 10});
    let report = json!({"function": IMPLEMENTATION, "direction": "inbound",
        "callers_total": 0, "callers": [], "has_more": false, "truncated": false, "next": null});
    let trace = record_trace(&mut lineages, &input, report).unwrap();
    assert_eq!(trace.root_binding, root);
    assert_eq!(
        trace.caller_discovery,
        Some(CallerDiscoveryOutcomeV1::NoProductionCallersReported)
    );
}

#[test]
fn incomplete_conflicting_or_malformed_reports_cannot_satisfy_caller_evidence() {
    let mut reports = Vec::new();
    let mut missing_total = empty_release_trace();
    missing_total
        .as_object_mut()
        .unwrap()
        .remove("callers_total");
    reports.push(missing_total);
    reports.push(json!({"function": IMPLEMENTATION, "callers": []}));
    for (field, value) in [
        ("callers_total", json!(1)),
        ("callers_total", json!("0")),
        ("callers_total", json!(-1)),
        ("callers", json!(null)),
        ("callers", json!(0)),
        ("callers", json!({})),
        (
            "callers",
            json!([{"qualified_name": "demo.src.delivery.dispatch"}]),
        ),
        ("callers", json!({"cols": [], "groups": []})),
        ("callers", json!({"cols": [42], "groups": []})),
        ("callers", json!({"cols": ["name", "name"], "groups": []})),
        (
            "callers",
            json!({"cols": ["name"], "groups": [{"qn_prefix": "demo", "rows": [], "truncated": true}]}),
        ),
        (
            "callers",
            json!({"cols": ["name"], "groups": [{"qn_prefix": "demo", "file": 42, "rows": []}]}),
        ),
        (
            "callers",
            json!({"cols": ["name"], "groups": [], "has_more": true}),
        ),
        ("has_more", json!(true)),
        ("has_more", json!("false")),
        ("truncated", json!(true)),
        ("truncated", json!(0)),
        ("nextCursor", json!("continuation")),
        ("next_cursor", json!(false)),
        ("direction", json!("outbound")),
        ("function", json!("demo.src.other.evaluate")),
        ("function", json!("demo.src.other.parse_policy")),
        ("function", json!("parse_policy")),
        ("include_tests", json!(true)),
        ("mode", json!("inheritance")),
    ] {
        let mut report = empty_release_trace();
        report[field] = value;
        reports.push(report);
    }
    for report in reports {
        let (mut lineages, _) = implementation_lineage(true);
        let trace = record_trace(&mut lineages, &trace_input(), report.clone());
        assert_ne!(
            trace.and_then(|trace| trace.caller_discovery),
            Some(CallerDiscoveryOutcomeV1::NoProductionCallersReported),
            "{report}"
        );
    }
}

#[test]
fn unselected_test_inclusive_and_conflicting_parts_cannot_report_zero_callers() {
    for (field, value) in [
        ("function_name", json!("unknown")),
        ("direction", json!("outbound")),
        ("function_name", json!("demo.src.other.parse_policy")),
        ("direction", json!(42)),
        ("include_tests", json!(true)),
        ("include_tests", json!("false")),
        ("mode", json!("inheritance")),
        ("edge_types", json!(["REFERENCES"])),
        ("edge_types", json!([])),
        ("depth", json!(0)),
        ("depth", json!("1")),
        ("limit", json!(0)),
        ("cursor", json!("continuation")),
        ("parameter_name", json!("policy")),
    ] {
        let (mut lineages, _) = implementation_lineage(true);
        let mut input = trace_input();
        input[field] = value;
        let trace = record_trace(&mut lineages, &input, empty_release_trace());
        assert_ne!(
            trace.and_then(|trace| trace.caller_discovery),
            Some(CallerDiscoveryOutcomeV1::NoProductionCallersReported),
            "{input}"
        );
    }
    let (mut lineages, _) = implementation_lineage(true);
    let mut result = normalized(empty_release_trace());
    result
        .typed_parts
        .as_mut()
        .unwrap()
        .extend(structured_parts(json!({
            "function": IMPLEMENTATION, "direction": "inbound", "callers": []
        })));
    let trace = lineages
        .record(
            &correlation(GraphCorrelationTargetKindV1::FunctionName),
            &trace_input(),
            result.typed_parts.as_deref(),
        )
        .unwrap();
    assert_eq!(
        trace.caller_discovery,
        Some(CallerDiscoveryOutcomeV1::NoEligibleSelector)
    );
}
