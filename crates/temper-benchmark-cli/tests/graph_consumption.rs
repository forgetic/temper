// SPDX-License-Identifier: MPL-2.0

use std::path::{Path, PathBuf};

use temper_benchmark_cli::{
    AnalyzeOptions, GraphConsumptionModeV1, GraphDecisionCorrelationV1, GraphDecisionKindV1,
    GraphDecisionTargetV1, MetricCoverageV1, NormalizedTrace, TraceDiagnosticCodeV1, analyze_trace,
    ingest_trace, render_run_summary_json, render_run_summary_markdown,
};
use temper_protocol_activity::{
    AgentActivityEventV1, AgentScopeKindV1, CapturedContentV1, GraphCorrelationTargetKindV1,
    GraphCorrelationToolV1, GraphCorrelationV1,
};

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
    producer: GraphDecisionCorrelationV1,
    consumption: Vec<GraphDecisionCorrelationV1>,
) -> GraphDecisionTargetV1 {
    GraphDecisionTargetV1 {
        target: target.to_string(),
        kind: GraphDecisionKindV1::Implementation,
        producer,
        consumption,
    }
}

fn graph_consumption_options() -> AnalyzeOptions {
    AnalyzeOptions {
        graph_decision_targets: vec![
            target(
                "worker_slot",
                correlation(
                    GraphCorrelationToolV1::SearchGraph,
                    GraphCorrelationTargetKindV1::QualifiedNamePattern,
                    "worker_slot",
                ),
                vec![correlation(
                    GraphCorrelationToolV1::SearchCode,
                    GraphCorrelationTargetKindV1::Pattern,
                    "worker_slot",
                )],
            ),
            target(
                "worker_slot",
                correlation(
                    GraphCorrelationToolV1::SearchCode,
                    GraphCorrelationTargetKindV1::Pattern,
                    "worker_slot",
                ),
                vec![correlation(
                    GraphCorrelationToolV1::TracePath,
                    GraphCorrelationTargetKindV1::FunctionName,
                    "worker_slot",
                )],
            ),
            target(
                "DeliveryAttempt",
                correlation(
                    GraphCorrelationToolV1::TracePath,
                    GraphCorrelationTargetKindV1::FunctionName,
                    "worker_slot",
                ),
                vec![correlation(
                    GraphCorrelationToolV1::GetCodeSnippet,
                    GraphCorrelationTargetKindV1::QualifiedName,
                    "DeliveryAttempt",
                )],
            ),
            target(
                "DeliveryRouter::worker_for",
                correlation(
                    GraphCorrelationToolV1::GetCodeSnippet,
                    GraphCorrelationTargetKindV1::QualifiedName,
                    "DeliveryAttempt",
                ),
                vec![correlation(
                    GraphCorrelationToolV1::GetCodeSnippet,
                    GraphCorrelationTargetKindV1::QualifiedName,
                    "DeliveryRouter::worker_for",
                )],
            ),
            target(
                "repo/src/route.rs",
                correlation(
                    GraphCorrelationToolV1::GetCodeSnippet,
                    GraphCorrelationTargetKindV1::QualifiedName,
                    "DeliveryRouter::worker_for",
                ),
                Vec::new(),
            ),
        ],
        ..AnalyzeOptions::default()
    }
}

fn graph_consumption_trace() -> NormalizedTrace {
    ingest_trace(fixture("graph-consumption-events.jsonl")).unwrap()
}

fn event_has_call(event: &AgentActivityEventV1, call_id: &str) -> bool {
    matches!(event, AgentActivityEventV1::ToolStarted(tool) if tool.call_id == call_id)
        || matches!(event, AgentActivityEventV1::ToolFinished(tool) if tool.call_id == call_id)
}

fn set_tool_name(trace: &mut NormalizedTrace, call_id: &str, name: &str) {
    for event in &mut trace.events {
        match &mut event.event {
            AgentActivityEventV1::ToolStarted(tool) if tool.call_id == call_id => {
                tool.name = name.to_string();
            }
            AgentActivityEventV1::ToolFinished(tool) if tool.call_id == call_id => {
                tool.name = name.to_string();
            }
            _ => {}
        }
    }
}

fn set_tool_scope(trace: &mut NormalizedTrace, call_id: &str, scope: &str) {
    for event in &mut trace.events {
        if event_has_call(&event.event, call_id) {
            event.scope.id = scope.to_string();
            event.scope.kind = AgentScopeKindV1::SubAgent;
            event.scope.parent_id = Some("main".to_string());
        }
    }
}

fn set_tool_sequence(trace: &mut NormalizedTrace, call_id: &str, sequence: u64) {
    for event in &mut trace.events {
        if event_has_call(&event.event, call_id) {
            event.seq = sequence;
        }
    }
}

fn set_started_arguments(
    trace: &mut NormalizedTrace,
    call_id: &str,
    arguments: &str,
    truncated: bool,
) {
    for event in &mut trace.events {
        let AgentActivityEventV1::ToolStarted(tool) = &mut event.event else {
            continue;
        };
        if tool.call_id != call_id {
            continue;
        }
        let Some(CapturedContentV1::Inline(content)) = tool.arguments.as_mut() else {
            panic!("fixture tool arguments must be inline");
        };
        content.text = arguments.to_string();
        content.truncated = truncated;
    }
}

fn set_finished_result(trace: &mut NormalizedTrace, call_id: &str, result: &str, truncated: bool) {
    for event in &mut trace.events {
        let AgentActivityEventV1::ToolFinished(tool) = &mut event.event else {
            continue;
        };
        if tool.call_id != call_id {
            continue;
        }
        let Some(CapturedContentV1::Inline(content)) = tool.result.as_mut() else {
            panic!("fixture graph result must be inline");
        };
        content.text = result.to_string();
        content.truncated = truncated;
    }
}

fn set_finished_correlation(
    trace: &mut NormalizedTrace,
    call_id: &str,
    tool: GraphCorrelationToolV1,
    target_kind: GraphCorrelationTargetKindV1,
    target: &str,
) {
    for event in &mut trace.events {
        let AgentActivityEventV1::ToolFinished(finished) = &mut event.event else {
            continue;
        };
        if finished.call_id == call_id {
            finished.graph_correlation = GraphCorrelationV1::new(tool, target_kind, target);
        }
    }
}

fn clear_finished_correlation(trace: &mut NormalizedTrace, call_id: &str) {
    for event in &mut trace.events {
        let AgentActivityEventV1::ToolFinished(finished) = &mut event.event else {
            continue;
        };
        if finished.call_id == call_id {
            finished.graph_correlation = None;
        }
    }
}

fn corrupt_finished_correlation(trace: &mut NormalizedTrace, call_id: &str) {
    for event in &mut trace.events {
        let AgentActivityEventV1::ToolFinished(finished) = &mut event.event else {
            continue;
        };
        if finished.call_id == call_id {
            finished.graph_correlation.as_mut().unwrap().target_digest = "truncated".to_string();
        }
    }
}

fn graph_counts(
    summary: &temper_benchmark_cli::RunSummaryV1,
) -> (Option<u64>, Option<u64>, MetricCoverageV1) {
    let graph = summary.metrics.graph.as_ref().unwrap();
    (
        graph.relevant_results,
        graph.irrelevant_successes,
        graph.relevance_coverage.clone(),
    )
}

#[test]
fn graph_consumption_uses_typed_correlation_for_a_generic_five_call_chain() {
    let summary = analyze_trace(&graph_consumption_trace(), &graph_consumption_options());
    let graph = summary.metrics.graph.as_ref().unwrap();
    assert_eq!((graph.calls, graph.succeeded), (5, 5));
    assert_eq!(
        (graph.relevant_results, graph.irrelevant_successes),
        (Some(5), Some(0))
    );
    assert_eq!(
        graph.relevance_coverage,
        MetricCoverageV1 {
            observed: 5,
            expected: Some(5),
        }
    );
    assert_eq!(
        graph
            .decision_evidence
            .iter()
            .map(|evidence| (evidence.graph_finish_seq, evidence.consumer_start_seq))
            .collect::<Vec<_>>(),
        vec![(3, 4), (5, 6), (7, 8), (9, 10), (11, 12)]
    );
    assert_eq!(
        graph
            .decision_evidence
            .iter()
            .map(|evidence| evidence.consumption_mode)
            .collect::<Vec<_>>(),
        vec![
            GraphConsumptionModeV1::Graph,
            GraphConsumptionModeV1::Graph,
            GraphConsumptionModeV1::Source,
            GraphConsumptionModeV1::Source,
            GraphConsumptionModeV1::Mutation,
        ]
    );

    let json = render_run_summary_json(&summary).unwrap();
    let markdown = render_run_summary_markdown(&summary);
    for raw_value in ["private source one", "diff --git", "qualified_name_pattern"] {
        assert!(!json.contains(raw_value), "summary retained {raw_value:?}");
        assert!(
            !markdown.contains(raw_value),
            "report retained {raw_value:?}"
        );
    }
    assert!(
        markdown.contains(
            "| Graph call | Order | Graph tool | Consumer | Tool | Mode | Target | Kind |"
        )
    );
}

#[test]
fn v1_correlation_remains_observable_when_no_complete_lineage_can_be_derived() {
    // Lineage is intentionally wrapper-local and absent from durable activity.
    // A truncated provider result therefore has no lineage, but its existing
    // closed V1 input correlation must still give relevance complete coverage.
    let mut trace = graph_consumption_trace();
    set_finished_result(
        &mut trace,
        "graph-search",
        "truncated provider result",
        true,
    );

    let summary = analyze_trace(&trace, &graph_consumption_options());
    assert_eq!(
        graph_counts(&summary),
        (
            Some(5),
            Some(0),
            MetricCoverageV1 {
                observed: 5,
                expected: Some(5),
            },
        )
    );
}

#[test]
fn sentinel_results_cannot_substitute_for_exact_typed_correlation() {
    let mut trace = graph_consumption_trace();
    set_finished_result(&mut trace, "graph-search", "search-graph-marker", false);
    set_finished_correlation(
        &mut trace,
        "graph-search",
        GraphCorrelationToolV1::SearchGraph,
        GraphCorrelationTargetKindV1::GraphQuery,
        "unmatched provider query",
    );

    let summary = analyze_trace(&trace, &graph_consumption_options());
    assert_eq!(
        graph_counts(&summary),
        (
            Some(4),
            Some(1),
            MetricCoverageV1 {
                observed: 5,
                expected: Some(5),
            },
        )
    );
    assert!(
        !render_run_summary_json(&summary)
            .unwrap()
            .contains("search-graph-marker")
    );
}

#[test]
fn graph_consumption_rejects_broad_mismatched_cross_scope_and_out_of_order_consumers() {
    let cases: Vec<(&str, Box<dyn Fn(&mut NormalizedTrace)>, (u64, u64))> = vec![
        (
            "broad producer",
            Box::new(|trace| {
                set_tool_name(trace, "graph-search", "codebase_memory_get_architecture")
            }),
            (4, 1),
        ),
        (
            "mismatched consumer",
            Box::new(|trace| {
                set_finished_correlation(
                    trace,
                    "graph-code",
                    GraphCorrelationToolV1::SearchCode,
                    GraphCorrelationTargetKindV1::Pattern,
                    "unmatched",
                )
            }),
            (3, 2),
        ),
        (
            "cross-scope consumer",
            Box::new(|trace| set_tool_scope(trace, "graph-code", "child")),
            (3, 2),
        ),
        (
            "out-of-order consumer",
            Box::new(|trace| {
                set_tool_sequence(trace, "graph-code", 3);
                set_tool_sequence(trace, "graph-search", 5);
            }),
            (4, 1),
        ),
    ];
    for (name, mutate, (expected_relevant, expected_irrelevant)) in cases {
        let mut trace = graph_consumption_trace();
        mutate(&mut trace);
        let summary = analyze_trace(&trace, &graph_consumption_options());
        assert_eq!(
            graph_counts(&summary),
            (
                Some(expected_relevant),
                Some(expected_irrelevant),
                MetricCoverageV1 {
                    observed: 5,
                    expected: Some(5),
                },
            ),
            "{name} must not confer relevance"
        );
    }
}

#[test]
fn graph_consumption_marks_missing_malformed_or_lossy_correlation_unavailable() {
    let cases: Vec<(&str, Box<dyn Fn(&mut NormalizedTrace)>, u64)> = vec![
        (
            "missing producer correlation",
            Box::new(|trace| clear_finished_correlation(trace, "graph-search")),
            4,
        ),
        (
            "truncated raw metadata without correlation",
            Box::new(|trace| {
                clear_finished_correlation(trace, "graph-search");
                set_finished_result(trace, "graph-search", "generic provider summary", true);
                set_started_arguments(trace, "graph-search", "", true);
            }),
            4,
        ),
        (
            "malformed consumer correlation",
            Box::new(|trace| corrupt_finished_correlation(trace, "graph-code")),
            3,
        ),
    ];
    for (name, mutate, observed) in cases {
        let mut trace = graph_consumption_trace();
        mutate(&mut trace);
        let summary = analyze_trace(&trace, &graph_consumption_options());
        let (relevant, irrelevant, coverage) = graph_counts(&summary);
        assert_eq!((relevant, irrelevant), (None, None), "{name}");
        assert_eq!(
            coverage,
            MetricCoverageV1 {
                observed,
                expected: Some(5),
            },
            "{name}"
        );
        assert!(summary.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == TraceDiagnosticCodeV1::GraphEvidenceUnavailable
                && diagnostic.message.contains("correlation")
        }));
    }
}

#[test]
fn graph_consumption_rejects_mismatched_or_absent_later_mutations() {
    let cases: Vec<(&str, Box<dyn Fn(&mut NormalizedTrace)>)> = vec![
        (
            "mismatched mutation",
            Box::new(|trace| {
                set_started_arguments(
                    trace,
                    "patch-route",
                    r#"{"patch":"diff --git a/repo/src/other.rs b/repo/src/other.rs"}"#,
                    false,
                )
            }),
        ),
        (
            "unconsumed result",
            Box::new(|trace| {
                trace
                    .events
                    .retain(|event| !event_has_call(&event.event, "patch-route"));
            }),
        ),
    ];
    for (name, mutate) in cases {
        let mut trace = graph_consumption_trace();
        mutate(&mut trace);
        let summary = analyze_trace(&trace, &graph_consumption_options());
        assert_eq!(
            graph_counts(&summary),
            (
                Some(4),
                Some(1),
                MetricCoverageV1 {
                    observed: 5,
                    expected: Some(5),
                },
            ),
            "{name} must not count a non-exact or missing mutation"
        );
    }
}

#[test]
fn graph_consumption_redacts_raw_provider_and_argument_values() {
    const SECRET: &str = "Authorization: Bearer BENCHMARK-GRAPH-SECRET";
    let mut trace = graph_consumption_trace();
    set_started_arguments(
        &mut trace,
        "graph-search",
        &format!(r#"{{"qn_pattern":"{SECRET}"}}"#),
        false,
    );
    set_finished_result(
        &mut trace,
        "graph-search",
        &format!(r#"{{"summary":"{SECRET}"}}"#),
        false,
    );
    let summary = analyze_trace(&trace, &graph_consumption_options());
    assert_eq!(graph_counts(&summary).0, Some(5));
    for rendered in [
        render_run_summary_json(&summary).unwrap(),
        render_run_summary_markdown(&summary),
    ] {
        assert!(!rendered.contains(SECRET));
    }
}
