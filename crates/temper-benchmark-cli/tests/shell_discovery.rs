// SPDX-License-Identifier: MPL-2.0

use std::path::{Path, PathBuf};

use temper_benchmark_cli::{
    AnalyzeOptions, DistributionV1, GraphDecisionCorrelationV1, GraphDecisionKindV1,
    GraphDecisionTargetV1, MetricCoverageV1, NormalizedTrace, TraceDiagnosticCodeV1,
    aggregate_run_summaries, analyze_trace, ingest_trace, render_run_summary_markdown,
};
use temper_protocol_activity::{
    AgentActivityEventV1, CapturedContentV1, GraphCorrelationTargetKindV1, GraphCorrelationToolV1,
    InlineContentV1, ToolFailureCategoryV1, ToolFailureDiagnosticV1, ToolFailureReasonV1,
    ToolFinishedV1, ToolStartedV1, ToolStatusV1,
};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

fn shell_discovery_options() -> AnalyzeOptions {
    AnalyzeOptions {
        discovery_command_prefixes: vec![
            vec!["git".to_string(), "grep".to_string()],
            vec!["rg".to_string()],
        ],
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
    }
}

fn shell_discovery_summary(name: &str) -> temper_benchmark_cli::RunSummaryV1 {
    analyze_trace(
        &ingest_trace(fixture(name)).unwrap(),
        &shell_discovery_options(),
    )
}

fn denied_start(trace: &mut NormalizedTrace) -> &mut ToolStartedV1 {
    trace
        .events
        .iter_mut()
        .find_map(|event| match &mut event.event {
            AgentActivityEventV1::ToolStarted(tool) if tool.name == "bash" => Some(tool),
            _ => None,
        })
        .unwrap()
}

fn denied_finish(trace: &mut NormalizedTrace) -> &mut ToolFinishedV1 {
    trace
        .events
        .iter_mut()
        .find_map(|event| match &mut event.event {
            AgentActivityEventV1::ToolFinished(tool) if tool.name == "bash" => Some(tool),
            _ => None,
        })
        .unwrap()
}

fn assert_denial_is_unavailable(trace: &NormalizedTrace, case: &str) {
    let summary = analyze_trace(trace, &shell_discovery_options());
    let discovery = summary
        .metrics
        .graph
        .as_ref()
        .unwrap()
        .conventional_discovery_before_selection
        .as_ref()
        .unwrap();
    assert_eq!(discovery.classified_shell_segments, 0, "{case}");
    assert_eq!(discovery.total_calls, None, "{case}");
    assert_eq!(
        discovery.shell_command_classification_coverage,
        MetricCoverageV1 {
            observed: 0,
            expected: Some(1),
        },
        "{case}"
    );
    assert!(summary.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == TraceDiagnosticCodeV1::GraphEvidenceUnavailable
            && diagnostic
                .message
                .contains("shell discovery classification is unavailable")
    }));
}

#[test]
fn compound_shell_discovery_counts_each_parseable_matching_segment() {
    let summary = shell_discovery_summary("graph-shell-compound-events.jsonl");
    let discovery = summary
        .metrics
        .graph
        .as_ref()
        .unwrap()
        .conventional_discovery_before_selection
        .as_ref()
        .unwrap();

    assert_eq!(discovery.classified_shell_segments, 3);
    assert_eq!(discovery.total_calls, Some(3));
    assert_eq!(
        discovery.shell_command_classification_coverage,
        MetricCoverageV1 {
            observed: 1,
            expected: Some(1),
        }
    );
    assert!(
        render_run_summary_markdown(&summary)
            .contains("| Rubric-classified shell discovery segments | 3 |")
    );
}

#[test]
fn excluded_shell_denial_preserves_later_decision_chain_and_discovery_eligibility() {
    let trace = ingest_trace(fixture("graph-metrics-excluded-denial-events.jsonl")).unwrap();
    let producer = |tool: GraphCorrelationToolV1,
                    target_kind: GraphCorrelationTargetKindV1,
                    target: &str| GraphDecisionCorrelationV1 {
        tool,
        target_kind,
        target: target.to_string(),
    };
    let summary = analyze_trace(
        &trace,
        &AnalyzeOptions {
            discovery_command_prefixes: vec![vec!["git".to_string(), "grep".to_string()]],
            graph_decision_targets: vec![
                GraphDecisionTargetV1 {
                    target: "src/lib.rs".to_string(),
                    kind: GraphDecisionKindV1::Implementation,
                    producer: producer(
                        GraphCorrelationToolV1::SearchGraph,
                        GraphCorrelationTargetKindV1::GraphQuery,
                        "src/lib.rs",
                    ),
                    consumption: Vec::new(),
                },
                GraphDecisionTargetV1 {
                    target: "src/main.rs".to_string(),
                    kind: GraphDecisionKindV1::Caller,
                    producer: producer(
                        GraphCorrelationToolV1::TracePath,
                        GraphCorrelationTargetKindV1::FunctionName,
                        "src/main.rs",
                    ),
                    consumption: Vec::new(),
                },
                GraphDecisionTargetV1 {
                    target: "tests/focused.rs".to_string(),
                    kind: GraphDecisionKindV1::FocusedTest,
                    producer: producer(
                        GraphCorrelationToolV1::SearchCode,
                        GraphCorrelationTargetKindV1::Pattern,
                        "tests/focused.rs",
                    ),
                    consumption: Vec::new(),
                },
            ],
            ..AnalyzeOptions::default()
        },
    );
    let graph = summary.metrics.graph.as_ref().unwrap();
    let denial_seq = trace
        .events
        .iter()
        .find_map(|event| match &event.event {
            AgentActivityEventV1::ToolStarted(tool) if tool.name == "bash" => Some(event.seq),
            _ => None,
        })
        .unwrap();
    let first_graph_seq = trace
        .events
        .iter()
        .find_map(|event| match &event.event {
            AgentActivityEventV1::ToolStarted(tool)
                if tool.name.starts_with("codebase_memory_") =>
            {
                Some(event.seq)
            }
            _ => None,
        })
        .unwrap();

    assert!(denial_seq < first_graph_seq);
    assert_eq!(
        (graph.relevant_results, graph.irrelevant_successes),
        (Some(3), Some(1))
    );
    assert_eq!(
        graph
            .decision_evidence
            .iter()
            .map(|evidence| evidence.kind)
            .collect::<Vec<_>>(),
        vec![
            GraphDecisionKindV1::Implementation,
            GraphDecisionKindV1::Caller,
            GraphDecisionKindV1::FocusedTest,
        ]
    );
    let discovery = graph
        .conventional_discovery_before_selection
        .as_ref()
        .unwrap();
    assert_eq!(
        (
            discovery.grep_calls,
            discovery.find_calls,
            discovery.read_calls
        ),
        (1, 1, 1)
    );
    assert_eq!(discovery.classified_shell_segments, 0);
    assert_eq!(discovery.total_calls, Some(3));
    assert_eq!(
        discovery.shell_command_classification_coverage,
        MetricCoverageV1 {
            observed: 1,
            expected: Some(1),
        }
    );
}

#[test]
fn excluded_denial_is_complete_zero_credit_coverage_and_aggregate_evidence() {
    let trace = ingest_trace(fixture("graph-shell-excluded-denial-events.jsonl")).unwrap();
    let start = trace
        .events
        .iter()
        .find_map(|event| match &event.event {
            AgentActivityEventV1::ToolStarted(tool) if tool.name == "bash" => Some(tool),
            _ => None,
        })
        .unwrap();
    assert_eq!(start.arguments, None);
    let retained_start = serde_json::to_string(start).unwrap();
    for private_field in ["\"arguments\"", "\"command\"", "\"argv\""] {
        assert!(!retained_start.contains(private_field));
    }

    let summary = analyze_trace(&trace, &shell_discovery_options());
    let discovery = summary
        .metrics
        .graph
        .as_ref()
        .unwrap()
        .conventional_discovery_before_selection
        .as_ref()
        .unwrap();
    assert_eq!(discovery.classified_shell_segments, 0);
    assert_eq!(discovery.total_calls, Some(0));
    assert_eq!(
        discovery.shell_command_classification_coverage,
        MetricCoverageV1 {
            observed: 1,
            expected: Some(1),
        }
    );

    let aggregate = aggregate_run_summaries([summary]).unwrap();
    assert_eq!(
        aggregate.metrics["conventional_discovery_calls_before_selection"],
        DistributionV1::from_values(vec![0]).unwrap()
    );
    assert_eq!(
        aggregate.metrics["conventional_shell_segments_before_selection"],
        DistributionV1::from_values(vec![0]).unwrap()
    );
}

#[test]
fn incomplete_inconsistent_and_forged_denial_pairs_remain_unavailable() {
    let canonical = || ingest_trace(fixture("graph-shell-excluded-denial-events.jsonl")).unwrap();

    let mut trace = canonical();
    trace.events.retain(|event| {
        !matches!(
            &event.event,
            AgentActivityEventV1::ToolFinished(tool) if tool.name == "bash"
        )
    });
    assert_denial_is_unavailable(&trace, "missing completion");

    let mut trace = canonical();
    denied_start(&mut trace).shell_discovery_disposition = None;
    assert_denial_is_unavailable(&trace, "absent disposition");

    let mut trace = canonical();
    denied_finish(&mut trace).call_id = "different-call".to_string();
    assert_denial_is_unavailable(&trace, "mismatched call identity");

    let mut trace = canonical();
    denied_finish(&mut trace).status = ToolStatusV1::Succeeded;
    assert_denial_is_unavailable(&trace, "inconsistent completion status");

    let mut trace = canonical();
    denied_finish(&mut trace).failure = None;
    assert_denial_is_unavailable(&trace, "missing failure");

    let mut trace = canonical();
    denied_finish(&mut trace).failure = Some(ToolFailureDiagnosticV1::with_reason(
        ToolFailureCategoryV1::PolicyDenial,
        ToolFailureReasonV1::AccessDenied,
    ));
    assert_denial_is_unavailable(&trace, "inconsistent failure reason");

    let mut trace = canonical();
    denied_start(&mut trace)
        .shell_discovery_disposition
        .as_mut()
        .unwrap()
        .version = 2;
    assert_denial_is_unavailable(&trace, "malformed disposition version");

    let mut trace = canonical();
    denied_start(&mut trace)
        .shell_discovery_disposition
        .as_mut()
        .unwrap()
        .matching_discovery_segments = 1;
    assert_denial_is_unavailable(&trace, "forged discovery credit");

    let mut trace = canonical();
    denied_start(&mut trace).arguments = Some(CapturedContentV1::Inline(InlineContentV1 {
        text: r#"{"command":"git grep must-not-count"}"#.to_string(),
        truncated: false,
    }));
    assert_denial_is_unavailable(&trace, "forged disposition with command");
}

#[test]
fn parseable_non_discovery_shell_commands_count_as_zero() {
    let summary = shell_discovery_summary("graph-shell-no-match-events.jsonl");
    let discovery = summary
        .metrics
        .graph
        .as_ref()
        .unwrap()
        .conventional_discovery_before_selection
        .as_ref()
        .unwrap();

    assert_eq!(discovery.classified_shell_segments, 0);
    assert_eq!(discovery.total_calls, Some(0));
    assert_eq!(discovery.shell_command_classification_coverage.observed, 1);
    assert!(summary.diagnostics.iter().all(|diagnostic| {
        !diagnostic
            .message
            .contains("shell discovery classification is unavailable")
    }));
}

#[test]
fn quoted_and_escaped_shell_commands_are_completely_classified() {
    for (fixture_name, expected_segments) in [
        ("graph-shell-quoted-events.jsonl", 2),
        ("graph-shell-escaped-events.jsonl", 3),
    ] {
        let summary = shell_discovery_summary(fixture_name);
        let discovery = summary
            .metrics
            .graph
            .as_ref()
            .unwrap()
            .conventional_discovery_before_selection
            .as_ref()
            .unwrap();

        assert_eq!(discovery.classified_shell_segments, expected_segments);
        assert_eq!(discovery.total_calls, Some(expected_segments));
        assert_eq!(discovery.shell_command_classification_coverage.observed, 1);
    }
}

#[test]
fn argv_is_classified_as_one_exact_command_without_reparsing_arguments() {
    let summary = shell_discovery_summary("graph-shell-argv-events.jsonl");
    let discovery = summary
        .metrics
        .graph
        .as_ref()
        .unwrap()
        .conventional_discovery_before_selection
        .as_ref()
        .unwrap();

    assert_eq!(discovery.classified_shell_segments, 1);
    assert_eq!(discovery.total_calls, Some(1));
    assert_eq!(discovery.shell_command_classification_coverage.observed, 1);
}

#[test]
fn complete_production_backtick_preview_remains_legacy_compatible() {
    let summary = shell_discovery_summary("graph-shell-backtick-events.jsonl");
    let discovery = summary
        .metrics
        .graph
        .as_ref()
        .unwrap()
        .conventional_discovery_before_selection
        .as_ref()
        .unwrap();

    assert_eq!(discovery.classified_shell_segments, 1);
    assert_eq!(discovery.total_calls, Some(1));
}

#[test]
fn truncated_and_redacted_shell_evidence_never_becomes_zero() {
    for fixture_name in [
        "graph-shell-truncated-events.jsonl",
        "graph-shell-redacted-events.jsonl",
    ] {
        let summary = shell_discovery_summary(fixture_name);
        let discovery = summary
            .metrics
            .graph
            .as_ref()
            .unwrap()
            .conventional_discovery_before_selection
            .as_ref()
            .unwrap();

        assert_eq!(discovery.classified_shell_segments, 0);
        assert_eq!(discovery.total_calls, None);
        assert_eq!(discovery.shell_command_classification_coverage.observed, 0);
        assert!(summary.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == TraceDiagnosticCodeV1::GraphEvidenceUnavailable
                && diagnostic
                    .message
                    .contains("omitted, truncated, redacted, or malformed")
        }));
    }
}

#[test]
fn one_ineligible_shell_call_omits_the_all_component_total() {
    let summary = shell_discovery_summary("graph-shell-mixed-coverage-events.jsonl");
    let discovery = summary
        .metrics
        .graph
        .as_ref()
        .unwrap()
        .conventional_discovery_before_selection
        .as_ref()
        .unwrap();

    assert_eq!(discovery.classified_shell_segments, 1);
    assert_eq!(discovery.total_calls, None);
    assert_eq!(
        discovery.shell_command_classification_coverage,
        MetricCoverageV1 {
            observed: 1,
            expected: Some(2),
        }
    );
}

#[test]
fn unsupported_and_incomplete_shell_commands_remain_unknown() {
    for (fixture_name, expected_message) in [
        (
            "graph-shell-unsupported-events.jsonl",
            "unsupported or ambiguous shell syntax",
        ),
        (
            "graph-shell-missing-arguments-events.jsonl",
            "missing arguments",
        ),
    ] {
        let summary = shell_discovery_summary(fixture_name);
        let discovery = summary
            .metrics
            .graph
            .as_ref()
            .unwrap()
            .conventional_discovery_before_selection
            .as_ref()
            .unwrap();

        assert_eq!(discovery.total_calls, None);
        assert_eq!(discovery.shell_command_classification_coverage.observed, 0);
        assert!(
            summary
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains(expected_message))
        );
    }
}
