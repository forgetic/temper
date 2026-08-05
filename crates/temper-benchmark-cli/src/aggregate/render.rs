// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeMap;

use super::{ADVISORY_METRICS, BenchmarkAggregateV1, DistributionV1, PRIMARY_METRICS};
use crate::BenchmarkModeV1;

pub fn render_aggregate_markdown(aggregate: &BenchmarkAggregateV1) -> String {
    let mut report = String::from("# Benchmark aggregate\n\n");
    if aggregate.mode == Some(BenchmarkModeV1::Harness) {
        report.push_str(
            "> **Harness result:** plumbing and structural evidence only; not representative LLM performance.\n\n",
        );
    }
    if let Some(benchmark) = &aggregate.benchmark {
        report.push_str(&format!("- Benchmark: `{}`\n", markdown_text(benchmark)));
    }
    report.push_str(&format!(
        "- Runs: {} ({} succeeded, {} failed, {} cancelled, {} incomplete)\n\n",
        aggregate.outcomes.total,
        aggregate.outcomes.succeeded,
        aggregate.outcomes.failed,
        aggregate.outcomes.cancelled,
        aggregate.outcomes.incomplete
    ));
    render_metric_table(
        &mut report,
        "Correctness, discovery, and structural metrics",
        PRIMARY_METRICS,
        &aggregate.metrics,
    );
    report.push_str(
        "## Advisory timings\n\nTiming values are advisory; graph, model, tool, and wall timings are not pass/fail gates.\n\n",
    );
    render_metric_rows(&mut report, ADVISORY_METRICS, &aggregate.metrics);
    report
}

fn render_metric_table(
    report: &mut String,
    heading: &str,
    metrics: &[&str],
    values: &BTreeMap<String, DistributionV1>,
) {
    report.push_str(&format!("## {heading}\n\n"));
    render_metric_rows(report, metrics, values);
}

fn render_metric_rows(
    report: &mut String,
    metrics: &[&str],
    values: &BTreeMap<String, DistributionV1>,
) {
    report.push_str("| Metric | Samples | Min | p25 | Median | p75 | Max |\n");
    report.push_str("| --- | ---: | ---: | ---: | ---: | ---: | ---: |\n");
    for name in metrics {
        if let Some(value) = values.get(*name) {
            report.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} | {} |\n",
                metric_label(name),
                value.count,
                value.min,
                value.p25,
                value.median,
                value.p75,
                value.max
            ));
        } else {
            report.push_str(&format!(
                "| {} | — | — | — | — | — | — |\n",
                metric_label(name)
            ));
        }
    }
    report.push('\n');
}

pub(crate) fn metric_label(name: &str) -> String {
    markdown_text(&name.replace('_', " "))
}

pub(crate) fn markdown_text(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace(['\r', '\n'], " ")
}
