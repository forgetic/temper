// SPDX-License-Identifier: MPL-2.0

use std::path::{Path, PathBuf};

use temper_benchmark_cli::{
    AnalyzeOptions, GraphDecisionCorrelationV1, GraphDecisionKindV1, GraphDecisionTargetV1,
    MetricCoverageV1, TraceDiagnosticCodeV1, analyze_trace, ingest_trace,
};
use temper_protocol_activity::{GraphCorrelationTargetKindV1, GraphCorrelationToolV1};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

#[test]
fn old_or_metadata_only_graph_traces_keep_evidence_unavailable() {
    let trace = ingest_trace(fixture("graph-missing-evidence-events.jsonl")).unwrap();
    let summary = analyze_trace(
        &trace,
        &AnalyzeOptions {
            graph_decision_targets: vec![GraphDecisionTargetV1 {
                target: "src/lib.rs".to_string(),
                kind: GraphDecisionKindV1::Implementation,
                producer: GraphDecisionCorrelationV1 {
                    tool: GraphCorrelationToolV1::SearchGraph,
                    target_kind: GraphCorrelationTargetKindV1::GraphQuery,
                    target: "src/lib.rs".to_string(),
                },
                consumption: Vec::new(),
            }],
            ..AnalyzeOptions::default()
        },
    );
    let graph = summary.metrics.graph.as_ref().unwrap();
    assert_eq!(graph.calls, 1);
    assert_eq!(graph.cumulative_readiness_wait_ms, None);
    assert_eq!(
        graph.readiness_wait_coverage,
        MetricCoverageV1 {
            observed: 0,
            expected: Some(1)
        }
    );
    assert_eq!(graph.relevant_results, None);
    assert_eq!(graph.irrelevant_successes, None);
    assert_eq!(
        graph.relevance_coverage,
        MetricCoverageV1 {
            observed: 0,
            expected: Some(1)
        }
    );
    assert!(graph.conventional_discovery_before_selection.is_none());
    assert!(summary.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == TraceDiagnosticCodeV1::GraphEvidenceUnavailable
            && diagnostic.message.contains("correlation record")
    }));
}
