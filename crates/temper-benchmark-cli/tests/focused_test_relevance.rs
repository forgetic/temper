// SPDX-License-Identifier: MPL-2.0

use std::path::{Path, PathBuf};

use temper_benchmark_cli::{
    AnalyzeOptions, GraphConsumptionModeV1, GraphDecisionCorrelationV1, GraphDecisionKindV1,
    GraphDecisionTargetV1, GraphEvidenceToolV1, NormalizedTrace, analyze_trace, ingest_trace,
    render_run_summary_json, render_run_summary_markdown,
};
use temper_protocol_activity::{
    AgentActivityEventV1, AgentScopeKindV1, DecisionAnchorLineageStageV1, DecisionAnchorLineageV1,
    DecisionAnchorTargetKindV1, DecisionEvidenceKindV1, FocusedTestDiscoveryOutcomeV1,
    GraphCorrelationTargetKindV1, GraphCorrelationToolV1, GraphCorrelationV1, ToolStatusV1,
};

const ROOT: &str = "00000000-0000-4000-8000-000000000031";
const OTHER_ROOT: &str = "00000000-0000-4000-8000-000000000032";
const PRODUCER: &str = "graph-search";
const GENERIC_SOURCE: &str = "graph-source-delivery";
const TEST_SOURCE: &str = "graph-source-worker";
const QUERY: &str = "alias retry behavioral regression";
const DECLARED_FOCUSED_QUERY: &str = "focused alias retry regression";
const TEST_SELECTOR: &str = "alias_retries_stay_on_the_original_ordered_worker";
const PROVIDER_TEST_SELECTOR: &str =
    "temper-v1-hash.tests.alias_retry.alias_retries_stay_on_the_original_ordered_worker";

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

fn focused_target() -> GraphDecisionTargetV1 {
    GraphDecisionTargetV1 {
        target: "repo/tests/alias_retry.rs".to_string(),
        kind: GraphDecisionKindV1::FocusedTest,
        producer: correlation(
            GraphCorrelationToolV1::SearchGraph,
            GraphCorrelationTargetKindV1::GraphQuery,
            DECLARED_FOCUSED_QUERY,
        ),
        consumption: vec![correlation(
            GraphCorrelationToolV1::GetCodeSnippet,
            GraphCorrelationTargetKindV1::QualifiedName,
            TEST_SELECTOR,
        )],
    }
}

fn focused_options() -> AnalyzeOptions {
    AnalyzeOptions {
        graph_decision_targets: vec![focused_target()],
        ..AnalyzeOptions::default()
    }
}

fn ranking_options() -> AnalyzeOptions {
    let mut targets = vec![GraphDecisionTargetV1 {
        target: "DeliveryAttempt".to_string(),
        kind: GraphDecisionKindV1::Implementation,
        producer: correlation(
            GraphCorrelationToolV1::SearchGraph,
            GraphCorrelationTargetKindV1::GraphQuery,
            QUERY,
        ),
        consumption: vec![correlation(
            GraphCorrelationToolV1::GetCodeSnippet,
            GraphCorrelationTargetKindV1::QualifiedName,
            "DeliveryAttempt",
        )],
    }];
    targets.push(focused_target());
    AnalyzeOptions {
        graph_decision_targets: targets,
        ..AnalyzeOptions::default()
    }
}

fn producer_lineage(
    root: &str,
    outcome: Option<FocusedTestDiscoveryOutcomeV1>,
) -> DecisionAnchorLineageV1 {
    DecisionAnchorLineageV1::new_with_route_metadata(
        root.to_string(),
        DecisionAnchorLineageStageV1::Root,
        DecisionAnchorTargetKindV1::GraphQuery,
        [DecisionAnchorTargetKindV1::QualifiedName],
        [],
        None,
        None,
        outcome,
    )
    .unwrap()
}

fn source_lineage(
    root: &str,
    kind: Option<DecisionEvidenceKindV1>,
    canonical_targets: &[&str],
) -> DecisionAnchorLineageV1 {
    DecisionAnchorLineageV1::new_with_route_metadata(
        root.to_string(),
        DecisionAnchorLineageStageV1::CarryForward,
        DecisionAnchorTargetKindV1::QualifiedName,
        [DecisionAnchorTargetKindV1::QualifiedName],
        canonical_targets
            .iter()
            .map(|target| GraphCorrelationV1::target_digest(target).unwrap()),
        kind,
        None,
        None,
    )
    .unwrap()
}

fn focused_trace() -> NormalizedTrace {
    let mut trace = ingest_trace(fixture("graph-consumption-events.jsonl")).unwrap();
    trace.events.retain(|event| match &event.event {
        AgentActivityEventV1::ToolStarted(tool) => {
            matches!(
                tool.call_id.as_str(),
                PRODUCER | GENERIC_SOURCE | TEST_SOURCE
            )
        }
        AgentActivityEventV1::ToolFinished(tool) => {
            matches!(
                tool.call_id.as_str(),
                PRODUCER | GENERIC_SOURCE | TEST_SOURCE
            )
        }
        _ => true,
    });
    set_correlation(
        &mut trace,
        PRODUCER,
        GraphCorrelationToolV1::SearchGraph,
        GraphCorrelationTargetKindV1::GraphQuery,
        QUERY,
    );
    set_lineage(
        &mut trace,
        PRODUCER,
        Some(producer_lineage(
            ROOT,
            Some(FocusedTestDiscoveryOutcomeV1::EligibleSelectorReturned),
        )),
    );
    set_lineage(
        &mut trace,
        GENERIC_SOURCE,
        Some(source_lineage(ROOT, None, &["DeliveryAttempt"])),
    );
    set_correlation(
        &mut trace,
        TEST_SOURCE,
        GraphCorrelationToolV1::GetCodeSnippet,
        GraphCorrelationTargetKindV1::QualifiedName,
        PROVIDER_TEST_SELECTOR,
    );
    set_lineage(
        &mut trace,
        TEST_SOURCE,
        Some(source_lineage(
            ROOT,
            Some(DecisionEvidenceKindV1::FocusedTest),
            &[TEST_SELECTOR],
        )),
    );
    trace
}

fn set_lineage(
    trace: &mut NormalizedTrace,
    call_id: &str,
    lineage: Option<DecisionAnchorLineageV1>,
) {
    for event in &mut trace.events {
        if let AgentActivityEventV1::ToolFinished(tool) = &mut event.event {
            if tool.call_id == call_id {
                tool.decision_anchor_lineage = lineage.clone();
            }
        }
    }
}

fn set_correlation(
    trace: &mut NormalizedTrace,
    call_id: &str,
    tool: GraphCorrelationToolV1,
    kind: GraphCorrelationTargetKindV1,
    target: &str,
) {
    for event in &mut trace.events {
        if let AgentActivityEventV1::ToolFinished(finished) = &mut event.event {
            if finished.call_id == call_id {
                finished.graph_correlation = GraphCorrelationV1::new(tool, kind, target);
            }
        }
    }
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

fn set_start_sequence(trace: &mut NormalizedTrace, call_id: &str, sequence: u64) {
    for event in &mut trace.events {
        if matches!(&event.event, AgentActivityEventV1::ToolStarted(tool) if tool.call_id == call_id)
        {
            event.seq = sequence;
        }
    }
}

fn set_status(trace: &mut NormalizedTrace, call_id: &str, status: ToolStatusV1) {
    for event in &mut trace.events {
        if let AgentActivityEventV1::ToolFinished(tool) = &mut event.event {
            if tool.call_id == call_id {
                tool.status = status;
            }
        }
    }
}

fn clear_correlation(trace: &mut NormalizedTrace, call_id: &str) {
    for event in &mut trace.events {
        if let AgentActivityEventV1::ToolFinished(tool) = &mut event.event {
            if tool.call_id == call_id {
                tool.graph_correlation = None;
            }
        }
    }
}

fn corrupt_correlation(trace: &mut NormalizedTrace, call_id: &str) {
    for event in &mut trace.events {
        if let AgentActivityEventV1::ToolFinished(tool) = &mut event.event {
            if tool.call_id == call_id {
                tool.graph_correlation.as_mut().unwrap().target_digest = "invalid".to_string();
            }
        }
    }
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

fn focused_source_evidence(
    trace: &NormalizedTrace,
    options: &AnalyzeOptions,
) -> Vec<temper_benchmark_cli::GraphDecisionEvidenceV1> {
    analyze_trace(trace, options)
        .metrics
        .graph
        .unwrap()
        .decision_evidence
        .into_iter()
        .filter(|evidence| {
            evidence.kind == GraphDecisionKindV1::FocusedTest
                && evidence.consumption_mode == GraphConsumptionModeV1::Source
        })
        .collect()
}

#[test]
fn typed_focused_test_source_outranks_generic_evidence_without_changing_counts() {
    let trace = focused_trace();
    let options = ranking_options();

    let mut untyped = trace.clone();
    set_lineage(
        &mut untyped,
        TEST_SOURCE,
        Some(source_lineage(ROOT, None, &[TEST_SELECTOR])),
    );
    let untyped_summary = analyze_trace(&untyped, &options);
    let untyped_graph = untyped_summary.metrics.graph.as_ref().unwrap();
    assert_eq!(untyped_graph.decision_evidence.len(), 1);
    assert_eq!(
        (
            untyped_graph.decision_evidence[0].kind,
            untyped_graph.decision_evidence[0].consumer_call_id.as_str(),
        ),
        (GraphDecisionKindV1::Implementation, GENERIC_SOURCE)
    );

    let summary = analyze_trace(&trace, &options);
    let graph = summary.metrics.graph.as_ref().unwrap();
    assert_eq!(graph.decision_evidence.len(), 1);
    let evidence = &graph.decision_evidence[0];
    assert_eq!(evidence.graph_call_id, PRODUCER);
    assert_eq!(evidence.consumer_call_id, TEST_SOURCE);
    assert_eq!(evidence.kind, GraphDecisionKindV1::FocusedTest);
    assert_eq!(evidence.consumption_mode, GraphConsumptionModeV1::Source);
    assert_eq!(evidence.consumer_tool, GraphEvidenceToolV1::GetCodeSnippet);
    assert_eq!(evidence.target, "repo/tests/alias_retry.rs");
    assert_eq!(
        (
            graph.relevant_results,
            graph.irrelevant_successes,
            graph.relevance_coverage.clone(),
        ),
        (
            untyped_graph.relevant_results,
            untyped_graph.irrelevant_successes,
            untyped_graph.relevance_coverage.clone(),
        )
    );

    for rendered in [
        render_run_summary_json(&summary).unwrap(),
        render_run_summary_markdown(&summary),
    ] {
        assert!(!rendered.contains(PROVIDER_TEST_SELECTOR));
        assert!(!rendered.contains("src/delivery.rs"));
        assert!(!rendered.contains("qualified_name"));
    }
}

#[test]
fn exact_focused_test_source_correlation_is_accepted() {
    let mut trace = focused_trace();
    set_correlation(
        &mut trace,
        TEST_SOURCE,
        GraphCorrelationToolV1::GetCodeSnippet,
        GraphCorrelationTargetKindV1::QualifiedName,
        TEST_SELECTOR,
    );
    set_lineage(
        &mut trace,
        TEST_SOURCE,
        Some(source_lineage(
            ROOT,
            Some(DecisionEvidenceKindV1::FocusedTest),
            &[],
        )),
    );
    assert_eq!(focused_source_evidence(&trace, &focused_options()).len(), 1);
}

type TraceMutation = Box<dyn Fn(&mut NormalizedTrace)>;

#[test]
fn focused_test_source_evidence_fails_closed_for_untrusted_shapes() {
    let cases: Vec<(&str, TraceMutation)> = vec![
        (
            "missing producer lineage",
            Box::new(|trace| set_lineage(trace, PRODUCER, None)),
        ),
        (
            "invalid producer lineage",
            Box::new(|trace| {
                let mut lineage = producer_lineage(
                    ROOT,
                    Some(FocusedTestDiscoveryOutcomeV1::EligibleSelectorReturned),
                );
                lineage.root_binding = "invalid".to_string();
                set_lineage(trace, PRODUCER, Some(lineage));
            }),
        ),
        (
            "missing eligible discovery",
            Box::new(|trace| set_lineage(trace, PRODUCER, Some(producer_lineage(ROOT, None)))),
        ),
        (
            "settled empty discovery",
            Box::new(|trace| {
                set_lineage(
                    trace,
                    PRODUCER,
                    Some(producer_lineage(
                        ROOT,
                        Some(FocusedTestDiscoveryOutcomeV1::NoEligibleSelector),
                    )),
                )
            }),
        ),
        (
            "missing producer correlation",
            Box::new(|trace| clear_correlation(trace, PRODUCER)),
        ),
        (
            "invalid producer correlation",
            Box::new(|trace| corrupt_correlation(trace, PRODUCER)),
        ),
        (
            "missing consumer lineage",
            Box::new(|trace| set_lineage(trace, TEST_SOURCE, None)),
        ),
        (
            "missing typed confirmation",
            Box::new(|trace| {
                set_lineage(
                    trace,
                    TEST_SOURCE,
                    Some(source_lineage(ROOT, None, &[TEST_SELECTOR])),
                )
            }),
        ),
        (
            "different typed purpose",
            Box::new(|trace| {
                set_lineage(
                    trace,
                    TEST_SOURCE,
                    Some(source_lineage(
                        ROOT,
                        Some(DecisionEvidenceKindV1::Caller),
                        &[TEST_SELECTOR],
                    )),
                )
            }),
        ),
        (
            "mismatched root",
            Box::new(|trace| {
                set_lineage(
                    trace,
                    TEST_SOURCE,
                    Some(source_lineage(
                        OTHER_ROOT,
                        Some(DecisionEvidenceKindV1::FocusedTest),
                        &[TEST_SELECTOR],
                    )),
                )
            }),
        ),
        (
            "mismatched scope",
            Box::new(|trace| set_scope(trace, TEST_SOURCE, "child")),
        ),
        (
            "out of order",
            Box::new(|trace| set_start_sequence(trace, TEST_SOURCE, 3)),
        ),
        (
            "transformed selector",
            Box::new(|trace| {
                set_correlation(
                    trace,
                    TEST_SOURCE,
                    GraphCorrelationToolV1::GetCodeSnippet,
                    GraphCorrelationTargetKindV1::QualifiedName,
                    "invented_test_selector",
                );
                set_lineage(
                    trace,
                    TEST_SOURCE,
                    Some(source_lineage(
                        ROOT,
                        Some(DecisionEvidenceKindV1::FocusedTest),
                        &["invented_test_selector"],
                    )),
                );
            }),
        ),
        (
            "missing consumer correlation",
            Box::new(|trace| clear_correlation(trace, TEST_SOURCE)),
        ),
        (
            "invalid consumer correlation",
            Box::new(|trace| corrupt_correlation(trace, TEST_SOURCE)),
        ),
        (
            "failed producer",
            Box::new(|trace| set_status(trace, PRODUCER, ToolStatusV1::Failed)),
        ),
        (
            "failed consumer",
            Box::new(|trace| set_status(trace, TEST_SOURCE, ToolStatusV1::Failed)),
        ),
        (
            "non-source consumer",
            Box::new(|trace| {
                set_tool_name(trace, TEST_SOURCE, "codebase_memory_search_code");
                set_correlation(
                    trace,
                    TEST_SOURCE,
                    GraphCorrelationToolV1::SearchCode,
                    GraphCorrelationTargetKindV1::Pattern,
                    TEST_SELECTOR,
                );
                set_lineage(
                    trace,
                    TEST_SOURCE,
                    DecisionAnchorLineageV1::new(
                        ROOT.to_string(),
                        DecisionAnchorLineageStageV1::CarryForward,
                        DecisionAnchorTargetKindV1::Pattern,
                        [DecisionAnchorTargetKindV1::QualifiedName],
                    ),
                );
            }),
        ),
    ];

    for (name, mutate) in cases {
        let mut trace = focused_trace();
        mutate(&mut trace);
        assert!(
            focused_source_evidence(&trace, &focused_options()).is_empty(),
            "{name} unexpectedly produced focused-test source evidence",
        );
    }
}

#[test]
fn equivalent_repeated_consumers_select_the_earliest_call_deterministically() {
    let mut trace = focused_trace();
    for call_id in [GENERIC_SOURCE, TEST_SOURCE] {
        set_correlation(
            &mut trace,
            call_id,
            GraphCorrelationToolV1::GetCodeSnippet,
            GraphCorrelationTargetKindV1::QualifiedName,
            TEST_SELECTOR,
        );
        set_lineage(
            &mut trace,
            call_id,
            Some(source_lineage(
                ROOT,
                Some(DecisionEvidenceKindV1::FocusedTest),
                &[],
            )),
        );
    }
    let evidence = focused_source_evidence(&trace, &focused_options());
    assert_eq!(evidence.len(), 1);
    assert_eq!(evidence[0].consumer_call_id, GENERIC_SOURCE);
}

#[test]
fn ambiguous_consumers_or_declarations_fail_closed() {
    let mut duplicate_consumer = focused_trace();
    set_correlation(
        &mut duplicate_consumer,
        GENERIC_SOURCE,
        GraphCorrelationToolV1::GetCodeSnippet,
        GraphCorrelationTargetKindV1::QualifiedName,
        TEST_SELECTOR,
    );
    set_lineage(
        &mut duplicate_consumer,
        GENERIC_SOURCE,
        Some(source_lineage(
            ROOT,
            Some(DecisionEvidenceKindV1::FocusedTest),
            &[],
        )),
    );
    assert!(focused_source_evidence(&duplicate_consumer, &focused_options()).is_empty());

    let trace = focused_trace();
    let mut duplicate_declaration = focused_target();
    duplicate_declaration
        .consumption
        .push(duplicate_declaration.consumption[0].clone());
    let duplicate_declaration_options = AnalyzeOptions {
        graph_decision_targets: vec![duplicate_declaration],
        ..AnalyzeOptions::default()
    };
    assert!(focused_source_evidence(&trace, &duplicate_declaration_options).is_empty());

    let target = focused_target();
    let duplicate_target_options = AnalyzeOptions {
        graph_decision_targets: vec![target.clone(), target],
        ..AnalyzeOptions::default()
    };
    assert!(focused_source_evidence(&trace, &duplicate_target_options).is_empty());
}
