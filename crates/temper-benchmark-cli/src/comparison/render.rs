// SPDX-License-Identifier: MPL-2.0

use super::{BenchmarkComparisonV1, ComparisonSubjectV1, MetricComparisonV1};
use crate::DistributionV1;
use crate::aggregate::{markdown_text, metric_label};

pub fn render_comparison_markdown(comparison: &BenchmarkComparisonV1) -> String {
    let mut report = String::from("# Benchmark comparison\n\n");
    report.push_str(
        "This comparison is report-only. Valid artifacts never fail because of metric deltas.\n\n",
    );
    render_subject(&mut report, "Base", &comparison.base);
    render_subject(&mut report, "Head", &comparison.head);
    report.push('\n');
    render_comparison_table(
        &mut report,
        "Primary correctness, discovery, and structural metrics",
        &comparison.primary,
    );
    report.push_str(
        "## Advisory timings\n\nGraph, model, tool, and wall timings are advisory and are not pass/fail gates.\n\n",
    );
    render_comparison_rows(&mut report, &comparison.advisory);
    if !comparison.other.is_empty() {
        report.push_str(
            "## Additional metrics\n\nUnrecognized metrics are retained for visibility but are not classified as primary or advisory.\n\n",
        );
        render_comparison_rows(&mut report, &comparison.other);
    }
    report
}

fn render_subject(report: &mut String, label: &str, subject: &ComparisonSubjectV1) {
    let benchmark = subject
        .benchmark
        .as_deref()
        .map(markdown_text)
        .unwrap_or_else(|| "unidentified benchmark".to_string());
    let condition = subject
        .condition
        .and_then(|condition| serde_json::to_value(condition).ok())
        .and_then(|value| value.as_str().map(str::to_string))
        .map_or_else(String::new, |condition| format!(", condition {condition}"));
    report.push_str(&format!(
        "- {label}: {benchmark}{condition}, {} run(s), {} succeeded\n",
        subject.run_count, subject.success_count
    ));
}

fn render_comparison_table(report: &mut String, heading: &str, rows: &[MetricComparisonV1]) {
    report.push_str(&format!("## {heading}\n\n"));
    render_comparison_rows(report, rows);
}

fn render_comparison_rows(report: &mut String, rows: &[MetricComparisonV1]) {
    report.push_str("| Metric | Base median | Head median | Delta |\n");
    report.push_str("| --- | ---: | ---: | ---: |\n");
    for row in rows {
        report.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            metric_label(&row.metric),
            display_statistics(row.base.as_ref()),
            display_statistics(row.head.as_ref()),
            display_delta(row.median_delta)
        ));
    }
    report.push('\n');
}

fn display_statistics(value: Option<&DistributionV1>) -> String {
    value.map_or_else(
        || "—".to_string(),
        |value| {
            if value.count == 1 {
                value.median.to_string()
            } else {
                format!("{} (n={})", value.median, value.count)
            }
        },
    )
}

fn display_delta(value: Option<i128>) -> String {
    value.map_or_else(
        || "—".to_string(),
        |value| {
            if value > 0 {
                format!("+{value}")
            } else {
                value.to_string()
            }
        },
    )
}
