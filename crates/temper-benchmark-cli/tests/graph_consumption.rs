// SPDX-License-Identifier: MPL-2.0

use std::path::{Path, PathBuf};

use temper_benchmark_cli::{
    AnalyzeOptions, GraphConsumptionModeV1, GraphDecisionConsumptionV1, GraphDecisionKindV1,
    GraphDecisionTargetV1, GraphEvidenceToolV1, MetricCoverageV1, NormalizedTrace,
    TraceDiagnosticCodeV1, analyze_trace, ingest_trace, render_run_summary_json,
    render_run_summary_markdown,
};
use temper_protocol_activity::{AgentActivityEventV1, AgentScopeKindV1, CapturedContentV1};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

fn graph_consumption_options() -> AnalyzeOptions {
    let target =
        |target: &str, result_contains: &str, consumption: Vec<GraphDecisionConsumptionV1>| {
            GraphDecisionTargetV1 {
                target: target.to_string(),
                kind: GraphDecisionKindV1::Implementation,
                result_contains: Some(result_contains.to_string()),
                consumption,
            }
        };
    AnalyzeOptions {
        graph_decision_targets: vec![
            target(
                "worker_slot",
                "search-graph-marker",
                vec![GraphDecisionConsumptionV1 {
                    tool: GraphEvidenceToolV1::SearchCode,
                    target: "worker_slot".to_string(),
                }],
            ),
            target(
                "worker_slot",
                "search-code-marker",
                vec![GraphDecisionConsumptionV1 {
                    tool: GraphEvidenceToolV1::TracePath,
                    target: "worker_slot".to_string(),
                }],
            ),
            target(
                "DeliveryAttempt",
                "trace-marker",
                vec![GraphDecisionConsumptionV1 {
                    tool: GraphEvidenceToolV1::GetCodeSnippet,
                    target: "DeliveryAttempt".to_string(),
                }],
            ),
            target(
                "worker_for",
                "delivery-source-marker",
                vec![GraphDecisionConsumptionV1 {
                    tool: GraphEvidenceToolV1::GetCodeSnippet,
                    target: "worker_for".to_string(),
                }],
            ),
            target("repo/src/route.rs", "worker-source-marker", Vec::new()),
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

fn set_finished_result_truncated(trace: &mut NormalizedTrace, call_id: &str) {
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
        content.truncated = true;
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
fn graph_consumption_requires_declared_ordered_same_scope_chain_and_redacts_raw_evidence() {
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
    assert_eq!(
        graph
            .decision_evidence
            .iter()
            .map(|evidence| evidence.target.as_str())
            .collect::<Vec<_>>(),
        vec![
            "worker_slot",
            "worker_slot",
            "DeliveryAttempt",
            "worker_for",
            "repo/src/route.rs",
        ]
    );
    assert!(
        graph
            .decision_evidence
            .iter()
            .all(|evidence| evidence.kind == GraphDecisionKindV1::Implementation)
    );

    let json = render_run_summary_json(&summary).unwrap();
    let markdown = render_run_summary_markdown(&summary);
    for raw_value in ["search-graph-marker", "private source one", "diff --git"] {
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
    assert!(markdown.contains(
        "| `graph-search` | 3 → 4 | search_graph | `graph-code` | search_code | graph |"
    ));
}

#[test]
fn graph_consumption_rejects_broad_unmatched_cross_scope_and_out_of_order_consumers() {
    let cases: Vec<(&str, Box<dyn Fn(&mut NormalizedTrace)>)> = vec![
        (
            "broad producer",
            Box::new(|trace| {
                set_tool_name(trace, "graph-search", "codebase_memory_get_architecture")
            }),
        ),
        (
            "unmatched consumer",
            Box::new(|trace| {
                set_started_arguments(trace, "graph-code", r#"{"pattern":"unmatched"}"#, false)
            }),
        ),
        (
            "cross-scope consumer",
            Box::new(|trace| set_tool_scope(trace, "graph-code", "child")),
        ),
        (
            "out-of-order consumer",
            Box::new(|trace| {
                set_tool_sequence(trace, "graph-code", 3);
                set_tool_sequence(trace, "graph-search", 5);
            }),
        ),
    ];
    for (name, mutate) in cases {
        let mut trace = graph_consumption_trace();
        mutate(&mut trace);
        let summary = analyze_trace(&trace, &graph_consumption_options());
        let (relevant, irrelevant, coverage) = graph_counts(&summary);
        let expected_relevant = if name == "cross-scope consumer" { 3 } else { 4 };
        let expected_irrelevant = 5 - expected_relevant;
        assert_eq!(
            (relevant, irrelevant, coverage),
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
fn graph_consumption_marks_missing_or_truncated_correlation_unavailable() {
    let cases: Vec<(&str, Box<dyn Fn(&mut NormalizedTrace)>)> = vec![
        (
            "missing consumer arguments",
            Box::new(|trace| set_started_arguments(trace, "graph-code", "", true)),
        ),
        (
            "truncated producer result",
            Box::new(|trace| set_finished_result_truncated(trace, "graph-trace")),
        ),
    ];
    for (name, mutate) in cases {
        let mut trace = graph_consumption_trace();
        mutate(&mut trace);
        let summary = analyze_trace(&trace, &graph_consumption_options());
        let (relevant, irrelevant, coverage) = graph_counts(&summary);
        assert_eq!((relevant, irrelevant), (None, None), "{name}");
        assert_eq!(
            coverage,
            MetricCoverageV1 {
                observed: 4,
                expected: Some(5),
            },
            "{name}"
        );
        assert!(summary.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == TraceDiagnosticCodeV1::GraphEvidenceUnavailable
                && (diagnostic.message.contains("relevance is unavailable")
                    || diagnostic
                        .message
                        .contains("declared consumer omits arguments"))
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
