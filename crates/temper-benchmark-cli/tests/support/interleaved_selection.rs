// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use temper_benchmark_cli::{
    AnalyzeOptions, GraphDecisionCorrelationV1, GraphDecisionKindV1, GraphDecisionTargetV1,
    NormalizedTrace, ingest_trace,
};
use temper_protocol_activity::{
    AgentActivityEventV1, AgentRunEventV1, CallerDiscoveryOutcomeV1, CapturedContentV1,
    DecisionAnchorLineageStageV1, DecisionAnchorLineageV1, DecisionAnchorTargetKindV1,
    DecisionEvidenceKindV1, FocusedTestDiscoveryOutcomeV1, GraphCorrelationTargetKindV1,
    GraphCorrelationToolV1, GraphCorrelationV1,
};

pub(crate) const ROOT: &str = "00000000-0000-4000-8000-000000000061";
pub(crate) const OTHER_ROOT: &str = "00000000-0000-4000-8000-000000000062";
pub(crate) const ROOT_PRODUCER: &str = "graph-search";
pub(crate) const FOCUSED_PRODUCER: &str = "graph-code";
pub(crate) const CALLER_GRAPH: &str = "graph-trace";
pub(crate) const CALLER_SOURCE: &str = "graph-source-delivery";
pub(crate) const IMPLEMENTATION_SOURCE: &str = "graph-source-worker";
pub(crate) const GENERIC_CONSUMER: &str = "graph-generic-route";
pub(crate) const FOREST_CONSUMER: &str = "graph-forest-route";
pub(crate) const PARALLEL_SEARCH: &str = "graph-parallel-search";
pub(crate) const PARALLEL_CODE: &str = "graph-parallel-code";
pub(crate) const IRRELEVANT_GRAPH: &str = "graph-unrelated";
pub(crate) const READ_ROUTE: &str = "read-route";

pub(crate) const FOCUSED_QUERY: &str = "private focused route regression query";
pub(crate) const TEST_SELECTOR: &str = "route_regression_updates_existing_worker";
pub(crate) const PROVIDER_TEST_SELECTOR: &str =
    "temper-v1-private.tests.route.route_regression_updates_existing_worker";
pub(crate) const GENERIC_SELECTOR: &str = "private generic route fallback";
pub(crate) const FOREST_SELECTOR: &str = "private route forest selector";
pub(crate) const PARALLEL_SEARCH_SELECTOR: &str = "private wider batch graph query";
pub(crate) const PARALLEL_CODE_SELECTOR: &str = "private wider batch code pattern";
pub(crate) const IRRELEVANT_SELECTOR: &str = "private unrelated graph query";
pub(crate) const MALFORMED_SELECTOR: &str = "private malformed implementation selector";
pub(crate) const PRIVATE_SOURCE: &str = "private provider source text must not enter summaries";
pub(crate) const EXACT_TARGET: &str = "repo/src/route.rs";

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

pub(crate) fn options() -> AnalyzeOptions {
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
                EXACT_TARGET,
                GraphDecisionKindV1::Implementation,
                correlation(
                    GraphCorrelationToolV1::GetCodeSnippet,
                    GraphCorrelationTargetKindV1::QualifiedName,
                    TEST_SELECTOR,
                ),
                vec![correlation(
                    GraphCorrelationToolV1::SearchCode,
                    GraphCorrelationTargetKindV1::Pattern,
                    GENERIC_SELECTOR,
                )],
            ),
        ],
        ..AnalyzeOptions::default()
    }
}

fn lineage(
    root: &str,
    stage: DecisionAnchorLineageStageV1,
    target_kind: DecisionAnchorTargetKindV1,
    result_kind: DecisionAnchorTargetKindV1,
    canonical_targets: &[&str],
    evidence_kind: Option<DecisionEvidenceKindV1>,
    caller_discovery: Option<CallerDiscoveryOutcomeV1>,
    focused_test_discovery: Option<FocusedTestDiscoveryOutcomeV1>,
) -> DecisionAnchorLineageV1 {
    DecisionAnchorLineageV1::new_with_route_metadata(
        root.to_string(),
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

fn graph_pair(
    start_template: &AgentRunEventV1,
    finish_template: &AgentRunEventV1,
    call_id: &str,
    provider_argument: &str,
    graph_correlation: GraphCorrelationV1,
    decision_anchor_lineage: DecisionAnchorLineageV1,
) -> (AgentRunEventV1, AgentRunEventV1) {
    let mut start = start_template.clone();
    let AgentActivityEventV1::ToolStarted(started) = &mut start.event else {
        panic!("start template must be tool.started");
    };
    started.call_id = call_id.to_string();
    started.name = graph_correlation.tool.public_name().to_string();
    replace_inline(&mut started.arguments, provider_argument);

    let mut finish = finish_template.clone();
    let AgentActivityEventV1::ToolFinished(finished) = &mut finish.event else {
        panic!("finish template must be tool.finished");
    };
    finished.call_id = call_id.to_string();
    finished.name = graph_correlation.tool.public_name().to_string();
    finished.graph_correlation = Some(graph_correlation);
    finished.decision_anchor_lineage = Some(decision_anchor_lineage);
    replace_inline(&mut finished.result, PRIVATE_SOURCE);
    (start, finish)
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
                IMPLEMENTATION_SOURCE => replace_inline(
                    &mut started.arguments,
                    &format!(r#"{{"qualified_name":"{PROVIDER_TEST_SELECTOR}"}}"#),
                ),
                "patch-route" => {
                    started.call_id = READ_ROUTE.to_string();
                    started.name = "read".to_string();
                    replace_inline(&mut started.arguments, EXACT_TARGET);
                }
                _ => {}
            },
            AgentActivityEventV1::ToolFinished(finished) => match finished.call_id.as_str() {
                ROOT_PRODUCER => {
                    finished.decision_anchor_lineage = Some(lineage(
                        ROOT,
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
                        ROOT,
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
                        ROOT,
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
                        ROOT,
                        DecisionAnchorLineageStageV1::CarryForward,
                        DecisionAnchorTargetKindV1::QualifiedName,
                        DecisionAnchorTargetKindV1::QualifiedName,
                        &[],
                        Some(DecisionEvidenceKindV1::Caller),
                        None,
                        None,
                    ));
                }
                IMPLEMENTATION_SOURCE => {
                    finished.graph_correlation = GraphCorrelationV1::new(
                        GraphCorrelationToolV1::GetCodeSnippet,
                        GraphCorrelationTargetKindV1::QualifiedName,
                        PROVIDER_TEST_SELECTOR,
                    );
                    finished.decision_anchor_lineage = Some(lineage(
                        ROOT,
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

    let start_template = events[1].clone();
    let finish_template = events[2].clone();
    let read_start = events[11].clone();
    let read_finish = events[12].clone();
    let terminal = events[13].clone();
    events.truncate(11);

    let pair = |call_id, argument, tool, target_kind, target, route_lineage| {
        graph_pair(
            &start_template,
            &finish_template,
            call_id,
            argument,
            GraphCorrelationV1::new(tool, target_kind, target).unwrap(),
            route_lineage,
        )
    };
    let (generic_start, generic_finish) = pair(
        GENERIC_CONSUMER,
        r#"{"pattern":"private generic route fallback"}"#,
        GraphCorrelationToolV1::SearchCode,
        GraphCorrelationTargetKindV1::Pattern,
        GENERIC_SELECTOR,
        lineage(
            ROOT,
            DecisionAnchorLineageStageV1::CarryForward,
            DecisionAnchorTargetKindV1::Pattern,
            DecisionAnchorTargetKindV1::QualifiedName,
            &[],
            None,
            None,
            None,
        ),
    );
    let (forest_start, forest_finish) = pair(
        FOREST_CONSUMER,
        r#"{"function_name":"private route forest selector"}"#,
        GraphCorrelationToolV1::TracePath,
        GraphCorrelationTargetKindV1::FunctionName,
        FOREST_SELECTOR,
        lineage(
            ROOT,
            DecisionAnchorLineageStageV1::CarryForward,
            DecisionAnchorTargetKindV1::FunctionName,
            DecisionAnchorTargetKindV1::QualifiedName,
            &[EXACT_TARGET],
            None,
            None,
            None,
        ),
    );
    let (irrelevant_start, irrelevant_finish) = pair(
        IRRELEVANT_GRAPH,
        r#"{"query":"private unrelated graph query"}"#,
        GraphCorrelationToolV1::SearchGraph,
        GraphCorrelationTargetKindV1::GraphQuery,
        IRRELEVANT_SELECTOR,
        lineage(
            OTHER_ROOT,
            DecisionAnchorLineageStageV1::Root,
            DecisionAnchorTargetKindV1::GraphQuery,
            DecisionAnchorTargetKindV1::QualifiedName,
            &[],
            None,
            None,
            None,
        ),
    );
    let (parallel_search_start, parallel_search_finish) = pair(
        PARALLEL_SEARCH,
        r#"{"query":"private wider batch graph query"}"#,
        GraphCorrelationToolV1::SearchGraph,
        GraphCorrelationTargetKindV1::GraphQuery,
        PARALLEL_SEARCH_SELECTOR,
        lineage(
            ROOT,
            DecisionAnchorLineageStageV1::CarryForward,
            DecisionAnchorTargetKindV1::GraphQuery,
            DecisionAnchorTargetKindV1::QualifiedName,
            &[],
            None,
            None,
            None,
        ),
    );
    let (parallel_code_start, parallel_code_finish) = pair(
        PARALLEL_CODE,
        r#"{"pattern":"private wider batch code pattern"}"#,
        GraphCorrelationToolV1::SearchCode,
        GraphCorrelationTargetKindV1::Pattern,
        PARALLEL_CODE_SELECTOR,
        lineage(
            ROOT,
            DecisionAnchorLineageStageV1::CarryForward,
            DecisionAnchorTargetKindV1::Pattern,
            DecisionAnchorTargetKindV1::QualifiedName,
            &[],
            None,
            None,
            None,
        ),
    );

    events.extend([
        generic_start,
        generic_finish,
        forest_start,
        forest_finish,
        irrelevant_start,
        irrelevant_finish,
        parallel_search_start,
        read_start,
        parallel_code_start,
        parallel_search_finish,
        read_finish,
        parallel_code_finish,
        terminal,
    ]);
    for (index, event) in events.iter_mut().enumerate() {
        let seq = index as u64 + 1;
        event.seq = seq;
        event.elapsed_ms = seq;
        event.occurred_at = format!("2026-08-07T00:00:{seq:02}Z");
    }
    events
}

#[derive(Clone, Copy)]
pub(crate) enum SerializedVariant {
    Live,
    IncompleteRead,
    MalformedProducerRoute,
}

pub(crate) fn ingest_serialized(variant: SerializedVariant) -> NormalizedTrace {
    let mut records = live_shape_events()
        .iter()
        .map(|event| serde_json::to_value(event).unwrap())
        .collect::<Vec<Value>>();
    match variant {
        SerializedVariant::Live => {}
        SerializedVariant::IncompleteRead => records.retain(|record| {
            record["event"]["type"] != "tool.finished"
                || record["event"]["data"]["call_id"] != READ_ROUTE
        }),
        SerializedVariant::MalformedProducerRoute => {
            let record = records
                .iter_mut()
                .find(|record| {
                    record["event"]["type"] == "tool.finished"
                        && record["event"]["data"]["call_id"] == IMPLEMENTATION_SOURCE
                })
                .unwrap();
            record["event"]["data"]["graph_correlation"]["target_digest"] =
                Value::String(GraphCorrelationV1::target_digest(MALFORMED_SELECTOR).unwrap());
            record["event"]["data"]["decision_anchor_lineage"]["root_binding"] =
                Value::String(OTHER_ROOT.to_string());
        }
    }

    for (index, record) in records.iter_mut().enumerate() {
        let seq = index as u64 + 1;
        record["seq"] = Value::from(seq);
        record["elapsed_ms"] = Value::from(seq);
        record["occurred_at"] = Value::String(format!("2026-08-07T00:00:{seq:02}Z"));
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
    ingest_trace(path).expect("serialized interleaved activity records ingest")
}

pub(crate) fn producer_row<'a>(
    summary: &'a temper_benchmark_cli::RunSummaryV1,
) -> &'a temper_benchmark_cli::GraphDecisionEvidenceV1 {
    let rows = summary
        .metrics
        .graph
        .as_ref()
        .unwrap()
        .decision_evidence
        .iter()
        .filter(|evidence| evidence.graph_call_id == IMPLEMENTATION_SOURCE)
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 1, "implementation producer must retain one row");
    rows[0]
}
