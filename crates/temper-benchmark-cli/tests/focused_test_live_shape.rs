// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use temper_benchmark_cli::{
    AnalyzeOptions, GraphConsumptionModeV1, GraphDecisionCorrelationV1, GraphDecisionKindV1,
    GraphDecisionTargetV1, MetricCoverageV1, NormalizedTrace, RunSummaryV1, analyze_trace,
    ingest_trace, render_run_summary_json, render_run_summary_markdown,
};
use temper_protocol_activity::{
    AgentActivityEventV1, AgentRunEventV1, CallerDiscoveryOutcomeV1, CapturedContentV1,
    DecisionAnchorLineageStageV1, DecisionAnchorLineageV1, DecisionAnchorTargetKindV1,
    DecisionEvidenceKindV1, FocusedTestDiscoveryOutcomeV1, GraphCorrelationTargetKindV1,
    GraphCorrelationToolV1, GraphCorrelationV1,
};

const ROOT: &str = "00000000-0000-4000-8000-000000000041";
const OTHER_ROOT: &str = "00000000-0000-4000-8000-000000000042";
const ROOT_PRODUCER: &str = "graph-search";
const FOCUSED_PRODUCER: &str = "graph-code";
const CALLER_GRAPH: &str = "graph-trace";
const CALLER_SOURCE: &str = "graph-source-delivery";
const FOCUSED_SOURCE: &str = "graph-source-worker";
const READ_ROUTE: &str = "read-route";
const FOCUSED_QUERY: &str = "focused route regression";
const TEST_SELECTOR: &str = "route_regression_updates_existing_worker";
const PROVIDER_TEST_SELECTOR: &str =
    "temper-v1-hash.tests.route.route_regression_updates_existing_worker";
const PRIVATE_SOURCE: &str = "private focused test source must not enter summaries";

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

fn correlation(
    tool: GraphCorrelationToolV1,
    target_kind: GraphCorrelationTargetKindV1,
    target: &str,
) -> GraphDecisionCorrelationV1 {
    GraphDecisionCorrelationV1 {
        tool,
        target_kind,
        target: target.to_string(),
    }
}

fn target(
    target: &str,
    kind: GraphDecisionKindV1,
    producer: GraphDecisionCorrelationV1,
    consumption: Vec<GraphDecisionCorrelationV1>,
) -> GraphDecisionTargetV1 {
    GraphDecisionTargetV1 {
        target: target.to_string(),
        kind,
        producer,
        consumption,
    }
}

fn options() -> AnalyzeOptions {
    let focused_producer = correlation(
        GraphCorrelationToolV1::SearchGraph,
        GraphCorrelationTargetKindV1::GraphQuery,
        FOCUSED_QUERY,
    );
    let focused_consumer = correlation(
        GraphCorrelationToolV1::GetCodeSnippet,
        GraphCorrelationTargetKindV1::QualifiedName,
        TEST_SELECTOR,
    );
    AnalyzeOptions {
        graph_decision_targets: vec![
            target(
                "worker_slot",
                GraphDecisionKindV1::Implementation,
                correlation(
                    GraphCorrelationToolV1::SearchGraph,
                    GraphCorrelationTargetKindV1::QualifiedNamePattern,
                    "worker_slot",
                ),
                vec![correlation(
                    GraphCorrelationToolV1::TracePath,
                    GraphCorrelationTargetKindV1::FunctionName,
                    "worker_slot",
                )],
            ),
            target(
                "repo/tests/route.rs",
                GraphDecisionKindV1::Implementation,
                focused_producer.clone(),
                vec![focused_consumer.clone()],
            ),
            target(
                "repo/tests/route.rs",
                GraphDecisionKindV1::FocusedTest,
                focused_producer,
                vec![focused_consumer],
            ),
            target(
                "DeliveryRouter::worker_for",
                GraphDecisionKindV1::Caller,
                correlation(
                    GraphCorrelationToolV1::TracePath,
                    GraphCorrelationTargetKindV1::FunctionName,
                    "worker_slot",
                ),
                vec![correlation(
                    GraphCorrelationToolV1::GetCodeSnippet,
                    GraphCorrelationTargetKindV1::QualifiedName,
                    "DeliveryRouter::worker_for",
                )],
            ),
            target(
                "repo/tests/route.rs",
                GraphDecisionKindV1::Implementation,
                correlation(
                    GraphCorrelationToolV1::GetCodeSnippet,
                    GraphCorrelationTargetKindV1::QualifiedName,
                    "DeliveryRouter::worker_for",
                ),
                vec![correlation(
                    GraphCorrelationToolV1::GetCodeSnippet,
                    GraphCorrelationTargetKindV1::QualifiedName,
                    TEST_SELECTOR,
                )],
            ),
            target(
                "repo/src/route.rs",
                GraphDecisionKindV1::Implementation,
                correlation(
                    GraphCorrelationToolV1::GetCodeSnippet,
                    GraphCorrelationTargetKindV1::QualifiedName,
                    TEST_SELECTOR,
                ),
                Vec::new(),
            ),
        ],
        ..AnalyzeOptions::default()
    }
}

fn lineage(
    stage: DecisionAnchorLineageStageV1,
    target_kind: DecisionAnchorTargetKindV1,
    result_kind: DecisionAnchorTargetKindV1,
    canonical_targets: &[&str],
    evidence_kind: Option<DecisionEvidenceKindV1>,
    caller_discovery: Option<CallerDiscoveryOutcomeV1>,
    focused_test_discovery: Option<FocusedTestDiscoveryOutcomeV1>,
) -> DecisionAnchorLineageV1 {
    DecisionAnchorLineageV1::new_with_route_metadata(
        ROOT.to_string(),
        stage,
        target_kind,
        [result_kind],
        canonical_targets
            .iter()
            .map(|target| GraphCorrelationV1::target_digest(target).unwrap()),
        evidence_kind,
        caller_discovery,
        focused_test_discovery,
    )
    .unwrap()
}

fn replace_inline(content: &mut Option<CapturedContentV1>, text: &str) {
    let Some(CapturedContentV1::Inline(content)) = content else {
        panic!("fixture content must be inline");
    };
    content.text = text.to_string();
    content.truncated = false;
}

fn live_shape_events() -> Vec<AgentRunEventV1> {
    let mut events = ingest_trace(fixture("graph-consumption-events.jsonl"))
        .unwrap()
        .events;
    for event in &mut events {
        match &mut event.event {
            AgentActivityEventV1::ToolStarted(started) => match started.call_id.as_str() {
                FOCUSED_PRODUCER => {
                    started.name = GraphCorrelationToolV1::SearchGraph
                        .public_name()
                        .to_string();
                    replace_inline(
                        &mut started.arguments,
                        &format!(r#"{{"query":"{FOCUSED_QUERY}"}}"#),
                    );
                }
                FOCUSED_SOURCE => replace_inline(
                    &mut started.arguments,
                    &format!(
                        r#"{{"qualified_name":"{PROVIDER_TEST_SELECTOR}","decision_evidence_kind":"focused_test"}}"#
                    ),
                ),
                "patch-route" => {
                    started.call_id = READ_ROUTE.to_string();
                    started.name = "read".to_string();
                    replace_inline(&mut started.arguments, "repo/src/route.rs");
                }
                _ => {}
            },
            AgentActivityEventV1::ToolFinished(finished) => match finished.call_id.as_str() {
                ROOT_PRODUCER => {
                    finished.decision_anchor_lineage = Some(lineage(
                        DecisionAnchorLineageStageV1::Root,
                        DecisionAnchorTargetKindV1::QualifiedNamePattern,
                        DecisionAnchorTargetKindV1::FunctionName,
                        &[],
                        None,
                        None,
                        None,
                    ));
                }
                FOCUSED_PRODUCER => {
                    finished.name = GraphCorrelationToolV1::SearchGraph
                        .public_name()
                        .to_string();
                    finished.graph_correlation = GraphCorrelationV1::new(
                        GraphCorrelationToolV1::SearchGraph,
                        GraphCorrelationTargetKindV1::GraphQuery,
                        FOCUSED_QUERY,
                    );
                    finished.decision_anchor_lineage = Some(lineage(
                        DecisionAnchorLineageStageV1::CarryForward,
                        DecisionAnchorTargetKindV1::GraphQuery,
                        DecisionAnchorTargetKindV1::QualifiedName,
                        &[],
                        None,
                        None,
                        Some(FocusedTestDiscoveryOutcomeV1::EligibleSelectorReturned),
                    ));
                    replace_inline(
                        &mut finished.result,
                        &format!(r#"{{"qualified_name":"{PROVIDER_TEST_SELECTOR}"}}"#),
                    );
                }
                CALLER_GRAPH => {
                    finished.decision_anchor_lineage = Some(lineage(
                        DecisionAnchorLineageStageV1::CarryForward,
                        DecisionAnchorTargetKindV1::FunctionName,
                        DecisionAnchorTargetKindV1::QualifiedName,
                        &[],
                        None,
                        Some(CallerDiscoveryOutcomeV1::EligibleSelectorReturned),
                        None,
                    ));
                }
                CALLER_SOURCE => {
                    finished.graph_correlation = GraphCorrelationV1::new(
                        GraphCorrelationToolV1::GetCodeSnippet,
                        GraphCorrelationTargetKindV1::QualifiedName,
                        "DeliveryRouter::worker_for",
                    );
                    finished.decision_anchor_lineage = Some(lineage(
                        DecisionAnchorLineageStageV1::CarryForward,
                        DecisionAnchorTargetKindV1::QualifiedName,
                        DecisionAnchorTargetKindV1::QualifiedName,
                        &[],
                        Some(DecisionEvidenceKindV1::Caller),
                        None,
                        None,
                    ));
                }
                FOCUSED_SOURCE => {
                    finished.graph_correlation = GraphCorrelationV1::new(
                        GraphCorrelationToolV1::GetCodeSnippet,
                        GraphCorrelationTargetKindV1::QualifiedName,
                        PROVIDER_TEST_SELECTOR,
                    );
                    finished.decision_anchor_lineage = Some(lineage(
                        DecisionAnchorLineageStageV1::CarryForward,
                        DecisionAnchorTargetKindV1::QualifiedName,
                        DecisionAnchorTargetKindV1::QualifiedName,
                        &[TEST_SELECTOR],
                        Some(DecisionEvidenceKindV1::FocusedTest),
                        None,
                        None,
                    ));
                    replace_inline(&mut finished.result, PRIVATE_SOURCE);
                }
                "patch-route" => {
                    finished.call_id = READ_ROUTE.to_string();
                    finished.name = "read".to_string();
                }
                _ => {}
            },
            _ => {}
        }
    }
    events
}

#[derive(Clone, Copy)]
enum SerializedVariant {
    Live,
    MissingTypedKind,
    MalformedLineage,
}

fn ingest_serialized(variant: SerializedVariant) -> NormalizedTrace {
    let mut records = live_shape_events()
        .iter()
        .map(|event| serde_json::to_value(event).unwrap())
        .collect::<Vec<Value>>();
    for record in &mut records {
        if record["event"]["type"] != "tool.finished"
            || record["event"]["data"]["call_id"] != FOCUSED_SOURCE
        {
            continue;
        }
        let lineage = record["event"]["data"]["decision_anchor_lineage"]
            .as_object_mut()
            .unwrap();
        match variant {
            SerializedVariant::Live => {}
            SerializedVariant::MissingTypedKind => {
                lineage.remove("decision_evidence_kind");
            }
            SerializedVariant::MalformedLineage => {
                lineage.insert(
                    "root_binding".to_string(),
                    Value::String(OTHER_ROOT.to_string()),
                );
            }
        }
    }

    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("events.jsonl");
    let mut jsonl = records
        .iter()
        .map(|record| serde_json::to_string(record).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    jsonl.push('\n');
    fs::write(&path, jsonl).unwrap();
    ingest_trace(path).expect("serialized live activity records ingest")
}

fn unrelated_counts(summary: &RunSummaryV1) -> (u64, Option<u64>, u64, u64, u64, u64, Option<u64>) {
    let tools = summary.metrics.tools.as_ref().unwrap();
    let graph = summary.metrics.graph.as_ref().unwrap();
    let structure = summary.metrics.structure.as_ref().unwrap();
    (
        summary.trace.events.observed,
        summary.trace.events.expected,
        tools.calls,
        tools.succeeded,
        graph.calls,
        graph.succeeded,
        structure.mutations,
    )
}

#[test]
fn serialized_live_shape_retains_typed_decisions_and_exact_selection_once_per_producer() {
    let summary = analyze_trace(&ingest_serialized(SerializedVariant::Live), &options());
    let graph = summary.metrics.graph.as_ref().unwrap();

    assert_eq!((graph.calls, graph.succeeded), (5, 5));
    assert_eq!(
        graph.typed_correlation_coverage,
        Some(MetricCoverageV1 {
            observed: 5,
            expected: Some(5),
        })
    );
    assert_eq!(
        graph.typed_lineage_coverage,
        Some(MetricCoverageV1 {
            observed: 5,
            expected: Some(5),
        })
    );
    assert_eq!(
        (
            graph.relevant_results,
            graph.irrelevant_successes,
            graph.relevance_coverage.clone(),
        ),
        (
            Some(5),
            Some(0),
            MetricCoverageV1 {
                observed: 5,
                expected: Some(5),
            },
        )
    );
    assert_eq!(
        graph
            .decision_evidence
            .iter()
            .map(|evidence| (
                evidence.graph_call_id.as_str(),
                evidence.consumer_call_id.as_str(),
                evidence.kind,
                evidence.consumption_mode,
                evidence.target.as_str(),
            ))
            .collect::<Vec<_>>(),
        vec![
            (
                ROOT_PRODUCER,
                CALLER_GRAPH,
                GraphDecisionKindV1::Implementation,
                GraphConsumptionModeV1::Graph,
                "worker_slot",
            ),
            (
                FOCUSED_PRODUCER,
                FOCUSED_SOURCE,
                GraphDecisionKindV1::FocusedTest,
                GraphConsumptionModeV1::Source,
                "repo/tests/route.rs",
            ),
            (
                CALLER_GRAPH,
                CALLER_SOURCE,
                GraphDecisionKindV1::Caller,
                GraphConsumptionModeV1::Source,
                "DeliveryRouter::worker_for",
            ),
            (
                CALLER_SOURCE,
                FOCUSED_SOURCE,
                GraphDecisionKindV1::Implementation,
                GraphConsumptionModeV1::Source,
                "repo/tests/route.rs",
            ),
            (
                FOCUSED_SOURCE,
                READ_ROUTE,
                GraphDecisionKindV1::Implementation,
                GraphConsumptionModeV1::Selection,
                "repo/src/route.rs",
            ),
        ]
    );
    assert_eq!(
        graph
            .decision_evidence
            .iter()
            .map(|evidence| evidence.graph_call_id.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        graph.decision_evidence.len(),
    );

    for rendered in [
        render_run_summary_json(&summary).unwrap(),
        render_run_summary_markdown(&summary),
    ] {
        assert!(rendered.contains("repo/src/route.rs"));
        for private in [
            FOCUSED_QUERY,
            TEST_SELECTOR,
            PROVIDER_TEST_SELECTOR,
            PRIVATE_SOURCE,
        ] {
            assert!(!rendered.contains(private), "summary retained {private:?}");
        }
    }
}

#[test]
fn serialized_untyped_or_malformed_variants_do_not_manufacture_focused_test_evidence() {
    let live = analyze_trace(&ingest_serialized(SerializedVariant::Live), &options());
    let expected_counts = unrelated_counts(&live);

    for (name, variant) in [
        ("missing typed kind", SerializedVariant::MissingTypedKind),
        ("malformed lineage", SerializedVariant::MalformedLineage),
    ] {
        let summary = analyze_trace(&ingest_serialized(variant), &options());
        let graph = summary.metrics.graph.as_ref().unwrap();
        assert!(
            graph
                .decision_evidence
                .iter()
                .all(|evidence| evidence.kind != GraphDecisionKindV1::FocusedTest),
            "{name} manufactured focused-test evidence",
        );
        assert_eq!(unrelated_counts(&summary), expected_counts, "{name}");
    }
}
