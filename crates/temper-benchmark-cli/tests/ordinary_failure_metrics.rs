// SPDX-License-Identifier: MPL-2.0

use std::path::{Path, PathBuf};

use temper_benchmark_cli::{
    AnalyzeOptions, ComparisonInput, MetricCoverageV1, aggregate_run_summaries, analyze_trace,
    compare_benchmarks, ingest_trace, render_aggregate_markdown, render_comparison_markdown,
    render_run_summary_json, render_run_summary_markdown,
};
use temper_protocol_activity::{AgentActivityEventV1, ToolFailureCategoryV1, ToolFailureReasonV1};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

#[test]
fn ordinary_failure_metrics_use_only_closed_diagnostics_and_exclude_graph_wrappers() {
    let trace = ingest_trace(fixture("ordinary-failure-events.jsonl")).unwrap();
    let summary = analyze_trace(&trace, &AnalyzeOptions::default());
    let ordinary = summary
        .metrics
        .tools
        .as_ref()
        .unwrap()
        .ordinary
        .as_ref()
        .unwrap();

    assert_eq!(
        (
            ordinary.calls,
            ordinary.succeeded,
            ordinary.failed,
            ordinary.cancelled
        ),
        (4, 1, 2, 1)
    );
    assert_eq!(
        ordinary.status_coverage,
        MetricCoverageV1 {
            observed: 4,
            expected: Some(4)
        }
    );
    assert_eq!(
        ordinary.failure_category_coverage,
        MetricCoverageV1 {
            observed: 3,
            expected: Some(3)
        }
    );
    assert_eq!(
        ordinary.failure_reason_coverage,
        MetricCoverageV1 {
            observed: 3,
            expected: Some(3)
        }
    );
    assert_eq!(
        ordinary.failures_by_category[&ToolFailureCategoryV1::ExecutionFailure],
        1
    );
    assert_eq!(
        ordinary.failures_by_category[&ToolFailureCategoryV1::CircuitRedirect],
        1
    );
    assert_eq!(
        ordinary.failures_by_category[&ToolFailureCategoryV1::Cancellation],
        1
    );
    assert!(
        !ordinary
            .failures_by_category
            .contains_key(&ToolFailureCategoryV1::ProviderProtocol)
    );
    assert_eq!(
        ordinary.failures_by_reason[&ToolFailureReasonV1::ToolReportedFailure],
        1
    );
    assert_eq!(
        ordinary.failures_by_reason[&ToolFailureReasonV1::RepeatedNonRetryable],
        1
    );
    assert_eq!(ordinary.repeated_failure_redirects, Some(1));

    let json = render_run_summary_json(&summary).unwrap();
    let markdown = render_run_summary_markdown(&summary);
    for private in [
        "Authorization: Bearer ORDINARY-SECRET",
        "/private/source/path.rs",
        "fn private_source()",
        "private host output",
        "process-local-fingerprint",
    ] {
        assert!(!json.contains(private), "JSON leaked {private:?}");
        assert!(!markdown.contains(private), "Markdown leaked {private:?}");
    }
    assert!(markdown.contains("### Ordinary failure classification"));
    assert!(markdown.contains("| Repeated-failure redirects | 1 |"));
}

#[test]
fn missing_ordinary_diagnostics_lower_coverage_and_suppress_repeat_counts() {
    let mut trace = ingest_trace(fixture("ordinary-failure-events.jsonl")).unwrap();
    let event = trace
        .events
        .iter_mut()
        .find(|event| {
            matches!(&event.event, AgentActivityEventV1::ToolFinished(tool) if tool.call_id == "ordinary-failure")
        })
        .unwrap();
    let AgentActivityEventV1::ToolFinished(tool) = &mut event.event else {
        unreachable!()
    };
    tool.failure = None;
    let seq = event.seq;

    let summary = analyze_trace(&trace, &AnalyzeOptions::default());
    let ordinary = summary
        .metrics
        .tools
        .as_ref()
        .unwrap()
        .ordinary
        .as_ref()
        .unwrap();
    assert_eq!(
        ordinary.failure_category_coverage,
        MetricCoverageV1 {
            observed: 2,
            expected: Some(3)
        }
    );
    assert_eq!(
        ordinary.failure_reason_coverage,
        MetricCoverageV1 {
            observed: 2,
            expected: Some(3)
        }
    );
    assert_eq!(ordinary.repeated_failure_redirects, None);
    assert!(summary.diagnostics.iter().any(|diagnostic| {
        diagnostic.code
            == temper_benchmark_cli::TraceDiagnosticCodeV1::OrdinaryToolEvidenceUnavailable
            && diagnostic.seq == Some(seq)
            && diagnostic.message.contains("closed failure diagnostic")
    }));
}

#[test]
fn ordinary_failure_evidence_flows_through_aggregate_and_comparison_tables() {
    let mut complete = analyze_trace(
        &ingest_trace(fixture("ordinary-failure-events.jsonl")).unwrap(),
        &AnalyzeOptions::default(),
    );
    complete.identity.run_id = "ordinary-complete".to_string();
    let mut partial = complete.clone();
    partial.identity.run_id = "ordinary-partial".to_string();
    let partial_metrics = partial
        .metrics
        .tools
        .as_mut()
        .unwrap()
        .ordinary
        .as_mut()
        .unwrap();
    partial_metrics.failure_category_coverage.observed = 2;
    partial_metrics.failure_reason_coverage.observed = 2;
    partial_metrics.repeated_failure_redirects = None;

    let base = aggregate_run_summaries([complete.clone(), partial]).unwrap();
    assert_eq!(base.metrics["ordinary_tool_calls"].count, 2);
    assert_eq!(base.metrics["ordinary_failed_tool_calls"].count, 2);
    assert_eq!(base.metrics["ordinary_failure_execution_failure"].count, 1);
    assert_eq!(base.metrics["ordinary_repeated_failure_redirects"].count, 1);
    let aggregate_markdown = render_aggregate_markdown(&base);
    assert!(aggregate_markdown.contains("| ordinary failure execution failure | 1 |"));
    assert!(aggregate_markdown.contains("| ordinary repeated failure redirects | 1 |"));

    let head = aggregate_run_summaries([complete]).unwrap();
    let comparison = compare_benchmarks(
        &ComparisonInput::Aggregate(base),
        &ComparisonInput::Aggregate(head),
    )
    .unwrap();
    let redirect = comparison
        .primary
        .iter()
        .find(|metric| metric.metric == "ordinary_repeated_failure_redirects")
        .unwrap();
    assert_eq!(redirect.base.as_ref().unwrap().count, 1);
    assert_eq!(redirect.head.as_ref().unwrap().count, 1);
    assert_eq!(redirect.median_delta, Some(0));
    assert!(
        render_comparison_markdown(&comparison)
            .contains("| ordinary repeated failure redirects | 1 | 1 | 0 |")
    );
}
