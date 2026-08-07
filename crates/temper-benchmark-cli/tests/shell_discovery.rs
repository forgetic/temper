// SPDX-License-Identifier: MPL-2.0

use std::path::{Path, PathBuf};

use temper_benchmark_cli::{
    AnalyzeOptions, GraphDecisionKindV1, GraphDecisionTargetV1, MetricCoverageV1,
    TraceDiagnosticCodeV1, analyze_trace, ingest_trace, render_run_summary_markdown,
};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

fn shell_discovery_summary(name: &str) -> temper_benchmark_cli::RunSummaryV1 {
    analyze_trace(
        &ingest_trace(fixture(name)).unwrap(),
        &AnalyzeOptions {
            discovery_command_prefixes: vec![
                vec!["git".to_string(), "grep".to_string()],
                vec!["rg".to_string()],
            ],
            graph_decision_targets: vec![GraphDecisionTargetV1 {
                target: "src/lib.rs".to_string(),
                kind: GraphDecisionKindV1::Implementation,
                result_contains: None,
            }],
            ..AnalyzeOptions::default()
        },
    )
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
fn quoted_and_escaped_shell_commands_remain_unknown() {
    for fixture_name in [
        "graph-shell-quoted-events.jsonl",
        "graph-shell-escaped-events.jsonl",
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
                && diagnostic.message.contains("quoting or escaping")
        }));
    }
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
