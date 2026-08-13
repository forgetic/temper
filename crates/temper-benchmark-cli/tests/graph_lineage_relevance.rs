// SPDX-License-Identifier: MPL-2.0

use std::path::{Path, PathBuf};

use temper_benchmark_cli::{
    AnalyzeOptions, GraphDecisionCorrelationV1, GraphDecisionKindV1, GraphDecisionTargetV1,
    MetricCoverageV1, NormalizedTrace, analyze_trace, ingest_trace,
};
use temper_protocol_activity::{
    AgentActivityEventV1, AgentScopeKindV1, DecisionAnchorLineageStageV1, DecisionAnchorLineageV1,
    DecisionAnchorTargetKindV1, GraphCorrelationTargetKindV1, GraphCorrelationToolV1, ToolStatusV1,
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

fn options() -> AnalyzeOptions {
    AnalyzeOptions {
        graph_decision_targets: vec![GraphDecisionTargetV1 {
            target: "worker_slot".to_string(),
            kind: GraphDecisionKindV1::Implementation,
            producer: correlation(
                GraphCorrelationToolV1::SearchGraph,
                GraphCorrelationTargetKindV1::QualifiedNamePattern,
                "worker_slot",
            ),
            consumption: vec![correlation(
                GraphCorrelationToolV1::SearchCode,
                GraphCorrelationTargetKindV1::Pattern,
                "worker_slot",
            )],
        }],
        ..AnalyzeOptions::default()
    }
}

fn trace() -> NormalizedTrace {
    let mut trace = ingest_trace(fixture("graph-consumption-events.jsonl")).unwrap();
    trace.events.retain(|event| match &event.event {
        AgentActivityEventV1::ToolStarted(tool) => {
            !tool.name.starts_with("codebase_memory_")
                || matches!(tool.call_id.as_str(), "graph-search" | "graph-code")
        }
        AgentActivityEventV1::ToolFinished(tool) => {
            !tool.name.starts_with("codebase_memory_")
                || matches!(tool.call_id.as_str(), "graph-search" | "graph-code")
        }
        _ => true,
    });
    let root = "00000000-0000-4000-8000-000000000011";
    for event in &mut trace.events {
        let AgentActivityEventV1::ToolFinished(finished) = &mut event.event else {
            continue;
        };
        if !matches!(finished.call_id.as_str(), "graph-search" | "graph-code") {
            continue;
        }
        let correlation = finished.graph_correlation.as_mut().unwrap();
        let stage = if finished.call_id == "graph-search" {
            correlation.target_digest =
                temper_protocol_activity::GraphCorrelationV1::target_digest(
                    "organic root selector",
                )
                .unwrap();
            DecisionAnchorLineageStageV1::Root
        } else {
            correlation.target_digest =
                temper_protocol_activity::GraphCorrelationV1::target_digest(
                    "organic carry selector",
                )
                .unwrap();
            DecisionAnchorLineageStageV1::CarryForward
        };
        finished.decision_anchor_lineage = DecisionAnchorLineageV1::new(
            root.to_string(),
            stage,
            DecisionAnchorTargetKindV1::from_graph_correlation(correlation.target_kind),
            [DecisionAnchorTargetKindV1::FunctionName],
        );
    }
    trace
}

fn mutate_call(trace: &mut NormalizedTrace, call_id: &str, mutator: impl Fn(&mut NormalizedTrace)) {
    assert!(trace.events.iter().any(|event| match &event.event {
        AgentActivityEventV1::ToolStarted(tool) => tool.call_id == call_id,
        AgentActivityEventV1::ToolFinished(tool) => tool.call_id == call_id,
        _ => false,
    }));
    mutator(trace);
}

fn set_scope(trace: &mut NormalizedTrace, call_id: &str, scope: &str) {
    for event in &mut trace.events {
        let matches = match &event.event {
            AgentActivityEventV1::ToolStarted(tool) => tool.call_id == call_id,
            AgentActivityEventV1::ToolFinished(tool) => tool.call_id == call_id,
            _ => false,
        };
        if matches {
            event.scope.id = scope.to_string();
            event.scope.kind = AgentScopeKindV1::SubAgent;
            event.scope.parent_id = Some("main".to_string());
        }
    }
}

fn set_sequence(trace: &mut NormalizedTrace, call_id: &str, sequence: u64) {
    for event in &mut trace.events {
        let matches = match &event.event {
            AgentActivityEventV1::ToolStarted(tool) => tool.call_id == call_id,
            AgentActivityEventV1::ToolFinished(tool) => tool.call_id == call_id,
            _ => false,
        };
        if matches {
            event.seq = sequence;
        }
    }
}

fn set_root(trace: &mut NormalizedTrace, call_id: &str, root: &str) {
    for event in &mut trace.events {
        if let AgentActivityEventV1::ToolFinished(finished) = &mut event.event {
            if finished.call_id == call_id {
                finished
                    .decision_anchor_lineage
                    .as_mut()
                    .unwrap()
                    .root_binding = root.to_string();
            }
        }
    }
}

fn set_status(trace: &mut NormalizedTrace, call_id: &str, status: ToolStatusV1) {
    for event in &mut trace.events {
        if let AgentActivityEventV1::ToolFinished(finished) = &mut event.event {
            if finished.call_id == call_id {
                finished.status = status;
            }
        }
    }
}

fn remove_call(trace: &mut NormalizedTrace, call_id: &str) {
    trace.events.retain(|event| match &event.event {
        AgentActivityEventV1::ToolStarted(tool) => tool.call_id != call_id,
        AgentActivityEventV1::ToolFinished(tool) => tool.call_id != call_id,
        _ => true,
    });
}

fn assert_counts(trace: &NormalizedTrace, relevant: u64, irrelevant: u64, successful: u64) {
    let summary = analyze_trace(trace, &options());
    let graph = summary.metrics.graph.unwrap();
    assert_eq!(graph.relevant_results, Some(relevant));
    assert_eq!(graph.irrelevant_successes, Some(irrelevant));
    assert_eq!(
        graph.relevance_coverage,
        MetricCoverageV1 {
            observed: successful,
            expected: Some(successful),
        }
    );
}

#[test]
fn ordered_same_scope_carry_forward_is_relevant_without_a_manifest_target_digest() {
    let trace = trace();
    assert_counts(&trace, 1, 1, 2);
    assert!(
        analyze_trace(&trace, &options())
            .metrics
            .graph
            .unwrap()
            .decision_evidence
            .is_empty()
    );
}

#[test]
fn unbound_or_untrusted_roots_cannot_make_an_organic_carry_forward_relevant() {
    let mut cases = Vec::new();
    cases.push((
        "missing",
        Box::new(|trace: &mut NormalizedTrace| remove_call(trace, "graph-search"))
            as Box<dyn Fn(&mut NormalizedTrace)>,
        1,
    ));
    cases.push((
        "later",
        Box::new(|trace| {
            set_sequence(trace, "graph-search", 6);
            set_sequence(trace, "graph-code", 3);
        }),
        2,
    ));
    cases.push((
        "scope",
        Box::new(|trace| set_scope(trace, "graph-search", "child")),
        2,
    ));
    cases.push((
        "binding",
        Box::new(|trace| set_root(trace, "graph-code", "00000000-0000-4000-8000-000000000012")),
        2,
    ));
    cases.push((
        "invalid",
        Box::new(|trace| set_root(trace, "graph-search", "invalid")),
        2,
    ));
    cases.push((
        "failed",
        Box::new(|trace| set_status(trace, "graph-search", ToolStatusV1::Failed)),
        1,
    ));

    for (name, mutate, successful) in cases {
        let mut trace = trace();
        mutate_call(&mut trace, "graph-search", mutate);
        assert_counts(&trace, 0, successful, successful);
        assert!(!name.is_empty());
    }
}

#[test]
fn missing_invalid_or_failed_carry_forwards_do_not_make_a_root_relevant() {
    let mut cases = Vec::new();
    cases.push((
        Box::new(|trace: &mut NormalizedTrace| remove_call(trace, "graph-code"))
            as Box<dyn Fn(&mut NormalizedTrace)>,
        1,
    ));
    cases.push((
        Box::new(|trace| set_root(trace, "graph-code", "invalid")),
        2,
    ));
    cases.push((
        Box::new(|trace| set_status(trace, "graph-code", ToolStatusV1::Failed)),
        1,
    ));

    for (mutate, successful) in cases {
        let mut trace = trace();
        mutate_call(&mut trace, "graph-code", mutate);
        assert_counts(&trace, 0, successful, successful);
    }
}
