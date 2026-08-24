// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeSet;

use temper_benchmark_cli::{
    GraphConsumptionModeV1, GraphDecisionKindV1, GraphEvidenceToolV1, MetricCoverageV1,
    analyze_trace, render_run_summary_json, render_run_summary_markdown,
};

#[path = "support/interleaved_selection.rs"]
mod interleaved_selection;
use interleaved_selection::*;

#[test]
fn serialized_interleaved_exact_read_wins_over_generic_and_forest_consumers() {
    let trace = ingest_serialized(SerializedVariant::Live);
    let summary = analyze_trace(&trace, &options());
    let graph = summary.metrics.graph.as_ref().unwrap();

    assert_eq!((graph.calls, graph.succeeded), (10, 10));
    assert_eq!(
        (graph.relevant_results, graph.irrelevant_successes),
        (Some(9), Some(1))
    );
    let complete = Some(MetricCoverageV1 {
        observed: 10,
        expected: Some(10),
    });
    assert_eq!(graph.relevance_coverage, complete.clone().unwrap());
    assert_eq!(graph.typed_correlation_coverage, complete);
    assert_eq!(
        graph.typed_lineage_coverage,
        Some(MetricCoverageV1 {
            observed: 10,
            expected: Some(10),
        })
    );
    assert_eq!(
        graph
            .decision_evidence
            .iter()
            .map(|evidence| (
                evidence.graph_call_id.as_str(),
                evidence.consumer_call_id.as_str(),
                evidence.consumer_tool,
                evidence.consumption_mode,
                evidence.target.as_str(),
            ))
            .collect::<Vec<_>>(),
        vec![
            (
                ROOT_PRODUCER,
                CALLER_GRAPH,
                GraphEvidenceToolV1::TracePath,
                GraphConsumptionModeV1::Graph,
                "worker_slot",
            ),
            (
                FOCUSED_PRODUCER,
                IMPLEMENTATION_SOURCE,
                GraphEvidenceToolV1::GetCodeSnippet,
                GraphConsumptionModeV1::Source,
                "repo/tests/route.rs",
            ),
            (
                CALLER_GRAPH,
                CALLER_SOURCE,
                GraphEvidenceToolV1::GetCodeSnippet,
                GraphConsumptionModeV1::Source,
                "DeliveryRouter::worker_for",
            ),
            (
                CALLER_SOURCE,
                IMPLEMENTATION_SOURCE,
                GraphEvidenceToolV1::GetCodeSnippet,
                GraphConsumptionModeV1::Source,
                "repo/tests/route.rs",
            ),
            (
                IMPLEMENTATION_SOURCE,
                READ_ROUTE,
                GraphEvidenceToolV1::Read,
                GraphConsumptionModeV1::Selection,
                EXACT_TARGET,
            ),
            (
                GENERIC_CONSUMER,
                FOREST_CONSUMER,
                GraphEvidenceToolV1::TracePath,
                GraphConsumptionModeV1::Graph,
                EXACT_TARGET,
            ),
        ]
    );
    let producer = producer_row(&summary);
    assert_eq!(producer.kind, GraphDecisionKindV1::Implementation);
    assert_eq!(producer.consumer_start_seq, 19);
    assert_eq!(
        graph
            .decision_evidence
            .iter()
            .map(|evidence| evidence.graph_call_id.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        graph.decision_evidence.len(),
    );

    let permitted_keys = BTreeSet::from([
        "consumer_call_id",
        "consumer_start_seq",
        "consumer_tool",
        "consumption_mode",
        "graph_call_id",
        "graph_finish_seq",
        "graph_tool",
        "kind",
        "target",
    ]);
    let declared_targets = BTreeSet::from([
        "worker_slot",
        "repo/tests/route.rs",
        "DeliveryRouter::worker_for",
        EXACT_TARGET,
    ]);
    for evidence in &graph.decision_evidence {
        let value = serde_json::to_value(evidence).unwrap();
        assert_eq!(
            value
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            permitted_keys,
        );
        assert!(declared_targets.contains(evidence.target.as_str()));
    }

    for rendered in [
        render_run_summary_json(&summary).unwrap(),
        render_run_summary_markdown(&summary),
    ] {
        assert!(rendered.contains(EXACT_TARGET));
        for private in [
            FOCUSED_QUERY,
            TEST_SELECTOR,
            PROVIDER_TEST_SELECTOR,
            GENERIC_SELECTOR,
            FOREST_SELECTOR,
            PARALLEL_SEARCH_SELECTOR,
            PARALLEL_CODE_SELECTOR,
            IRRELEVANT_SELECTOR,
            MALFORMED_SELECTOR,
            PRIVATE_SOURCE,
            ROOT,
            OTHER_ROOT,
            "\"arguments\"",
            "canonical_target_digests",
            "decision_anchor_lineage",
            "root_binding",
        ] {
            assert!(!rendered.contains(private), "summary retained {private:?}");
        }
    }
}

#[test]
fn serialized_incomplete_or_malformed_routes_cannot_manufacture_exact_selection() {
    let incomplete = analyze_trace(
        &ingest_serialized(SerializedVariant::IncompleteRead),
        &options(),
    );
    let generic = producer_row(&incomplete);
    assert_eq!(generic.consumer_call_id, GENERIC_CONSUMER);
    assert_eq!(generic.consumption_mode, GraphConsumptionModeV1::Graph);

    let mut forest_options = options();
    forest_options
        .graph_decision_targets
        .iter_mut()
        .find(|target| target.target == EXACT_TARGET)
        .unwrap()
        .consumption
        .clear();
    let forest = analyze_trace(
        &ingest_serialized(SerializedVariant::IncompleteRead),
        &forest_options,
    );
    let forest = producer_row(&forest);
    assert_eq!(forest.consumer_call_id, FOREST_CONSUMER);
    assert_eq!(forest.consumption_mode, GraphConsumptionModeV1::Graph);

    let malformed = analyze_trace(
        &ingest_serialized(SerializedVariant::MalformedProducerRoute),
        &options(),
    );
    for (name, summary) in [
        ("incomplete read", &incomplete),
        ("malformed route", &malformed),
    ] {
        assert!(
            summary
                .metrics
                .graph
                .as_ref()
                .unwrap()
                .decision_evidence
                .iter()
                .all(|evidence| {
                    evidence.graph_call_id != IMPLEMENTATION_SOURCE
                        || evidence.consumption_mode != GraphConsumptionModeV1::Selection
                }),
            "{name} manufactured exact selection",
        );
    }
}
