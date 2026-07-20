// SPDX-License-Identifier: MPL-2.0

//! Stable JSON and Markdown rendering for typed run summaries.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;
use thiserror::Error;

use crate::{
    MetricCoverageV1, RunSummaryV1, RunTerminalStatusV1, StructureMetricsV1, ToolMetricsV1,
};

pub const RUN_SUMMARY_JSON_FILE: &str = "run.json";
pub const RUN_SUMMARY_MARKDOWN_FILE: &str = "run.md";

#[derive(Debug, Error)]
pub enum ReportWriteError {
    #[error("{operation} {}: {source}", path.display())]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("serialize run summary: {0}")]
    Serialization(#[from] serde_json::Error),
}

/// Renders deterministic pretty JSON with one trailing newline.
pub fn render_run_summary_json(summary: &RunSummaryV1) -> Result<String, serde_json::Error> {
    let mut rendered = serde_json::to_string_pretty(summary)?;
    rendered.push('\n');
    Ok(rendered)
}

/// Writes the stable run-summary pair to an artifact directory.
pub fn write_run_summary(
    summary: &RunSummaryV1,
    output_dir: impl AsRef<Path>,
) -> Result<(), ReportWriteError> {
    let output_dir = output_dir.as_ref();
    fs::create_dir_all(output_dir).map_err(|source| ReportWriteError::Io {
        operation: "create analysis output directory",
        path: output_dir.to_path_buf(),
        source,
    })?;
    write_file(
        &output_dir.join(RUN_SUMMARY_JSON_FILE),
        render_run_summary_json(summary)?.as_bytes(),
    )?;
    write_file(
        &output_dir.join(RUN_SUMMARY_MARKDOWN_FILE),
        render_run_summary_markdown(summary).as_bytes(),
    )
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), ReportWriteError> {
    fs::write(path, bytes).map_err(|source| ReportWriteError::Io {
        operation: "write analysis artifact",
        path: path.to_path_buf(),
        source,
    })
}

/// Renders a human report from the typed summary (never by scraping JSON or
/// command output).
pub fn render_run_summary_markdown(summary: &RunSummaryV1) -> String {
    let mut out = String::new();
    writeln!(out, "# Agent session benchmark\n").unwrap();
    writeln!(out, "## Run\n").unwrap();
    writeln!(out, "| Field | Value |").unwrap();
    writeln!(out, "| --- | --- |").unwrap();
    row(&mut out, "Summary version", summary.version);
    row(&mut out, "Run ID", code(&summary.identity.run_id));
    row(
        &mut out,
        "Repository",
        code(&summary.identity.assignment.repository),
    );
    row(
        &mut out,
        "Artifact",
        code(&summary.identity.assignment.artifact_ref),
    );
    row(&mut out, "Trace source", enum_label(&summary.source));
    row(
        &mut out,
        "Capture",
        summary
            .capture
            .as_ref()
            .map(enum_label)
            .unwrap_or_else(unavailable),
    );
    row(&mut out, "Events", coverage(&summary.trace.events));
    row(
        &mut out,
        "Attachments",
        coverage(&summary.trace.attachments),
    );
    out.push('\n');

    writeln!(out, "## Outcome\n").unwrap();
    writeln!(out, "| Metric | Value |").unwrap();
    writeln!(out, "| --- | ---: |").unwrap();
    match &summary.terminal {
        Some(terminal) => {
            row(
                &mut out,
                "Status",
                match terminal.status {
                    RunTerminalStatusV1::Succeeded => "succeeded",
                    RunTerminalStatusV1::Cancelled => "cancelled",
                    RunTerminalStatusV1::Failed => "failed",
                },
            );
            let reason = terminal
                .stop_reason
                .as_ref()
                .map(enum_label)
                .or_else(|| {
                    terminal.failure.as_ref().map(|failure| {
                        format!(
                            "{}: {}",
                            enum_label(&failure.code),
                            escape_cell(&failure.message)
                        )
                    })
                })
                .unwrap_or_else(unavailable);
            row(&mut out, "Reason", reason);
        }
        None => {
            row(&mut out, "Status", unavailable());
            row(&mut out, "Reason", unavailable());
        }
    }
    row(&mut out, "Wall time", optional_ms(summary.wall_time_ms));
    row(&mut out, "Turns", optional_count(summary.metrics.turns));
    out.push('\n');

    render_model_and_tokens(summary, &mut out);
    render_tools(summary.metrics.tools.as_ref(), &mut out);
    render_structure(summary.metrics.structure.as_ref(), &mut out);
    render_diagnostics(summary, &mut out);
    out
}

fn render_model_and_tokens(summary: &RunSummaryV1, out: &mut String) {
    writeln!(out, "## Model and tokens\n").unwrap();
    writeln!(out, "| Metric | Value |").unwrap();
    writeln!(out, "| --- | ---: |").unwrap();
    if let Some(model) = &summary.metrics.model {
        row(out, "Distinct calls", model.calls);
        row(out, "Provider attempts", model.attempts);
        row(out, "Succeeded attempts", model.succeeded_attempts);
        row(out, "Failed attempts", model.failed_attempts);
        row(out, "Cancelled attempts", model.cancelled_attempts);
        row(out, "Retries", model.retries);
        row(out, "Provider failures", model.provider_failures);
        row(
            out,
            "Cumulative model time",
            optional_ms(model.cumulative_duration_ms),
        );
        row(
            out,
            "Model time coverage",
            coverage(&model.duration_coverage),
        );
        row(
            out,
            "Cumulative TTFT",
            optional_ms(model.cumulative_time_to_first_token_ms),
        );
        row(
            out,
            "TTFT coverage",
            coverage(&model.time_to_first_token_coverage),
        );
    } else {
        row(out, "Model metrics", unavailable());
    }
    if let Some(tokens) = &summary.metrics.tokens {
        row(out, "Input tokens", tokens.input_tokens);
        row(out, "Output tokens", tokens.output_tokens);
        row(out, "Cache-read tokens", tokens.cache_read_tokens);
        row(out, "Cache-write tokens", tokens.cache_write_tokens);
        row(out, "Token coverage", coverage(&tokens.coverage));
    } else {
        row(out, "Token metrics", unavailable());
    }
    out.push('\n');
}

fn render_tools(tools: Option<&ToolMetricsV1>, out: &mut String) {
    writeln!(out, "## Tools\n").unwrap();
    let Some(tools) = tools else {
        writeln!(out, "_Unavailable._\n").unwrap();
        return;
    };
    writeln!(
        out,
        "| Tool | Calls | Succeeded | Failed | Cancelled | Duration | Coverage |"
    )
    .unwrap();
    writeln!(out, "| --- | ---: | ---: | ---: | ---: | ---: | ---: |").unwrap();
    for (name, metrics) in &tools.by_name {
        writeln!(
            out,
            "| {} | {} | {} | {} | {} | {} | {} |",
            escape_cell(name),
            metrics.calls,
            metrics.succeeded,
            metrics.failed,
            metrics.cancelled,
            optional_ms(metrics.cumulative_duration_ms),
            coverage(&metrics.duration_coverage),
        )
        .unwrap();
    }
    writeln!(
        out,
        "| **Total** | **{}** | **{}** | **{}** | **{}** | **{}** | **{}** |\n",
        tools.calls,
        tools.succeeded,
        tools.failed,
        tools.cancelled,
        optional_ms(tools.cumulative_duration_ms),
        coverage(&tools.duration_coverage),
    )
    .unwrap();

    writeln!(out, "### Slowest calls\n").unwrap();
    if tools.slowest.is_empty() {
        writeln!(out, "_No completed tool calls._\n").unwrap();
    } else {
        writeln!(out, "| Rank | Tool | Call ID | Duration |").unwrap();
        writeln!(out, "| ---: | --- | --- | ---: |").unwrap();
        for (index, call) in tools.slowest.iter().enumerate() {
            writeln!(
                out,
                "| {} | {} | {} | {} ms |",
                index + 1,
                escape_cell(&call.name),
                code(&call.call_id),
                call.duration_ms,
            )
            .unwrap();
        }
        out.push('\n');
    }
}

fn render_structure(structure: Option<&StructureMetricsV1>, out: &mut String) {
    writeln!(out, "## Mutation and validation structure\n").unwrap();
    writeln!(out, "| Metric | Value |").unwrap();
    writeln!(out, "| --- | ---: |").unwrap();
    if let Some(structure) = structure {
        row(
            out,
            "Failed edit attempts",
            optional_count(structure.failed_edit_attempts),
        );
        row(
            out,
            "Write/edit mutations",
            optional_count(structure.mutations),
        );
        row(
            out,
            "Validation boundaries",
            optional_count(structure.validation_boundaries),
        );
        row(
            out,
            "Post-validation mutations",
            optional_count(structure.post_validation_mutations),
        );
        row(
            out,
            "Validation invalidations",
            optional_count(structure.validation_invalidations),
        );
        row(
            out,
            "Revalidations",
            optional_count(structure.revalidations),
        );
    } else {
        row(out, "Structure metrics", unavailable());
    }
    out.push('\n');
}

fn render_diagnostics(summary: &RunSummaryV1, out: &mut String) {
    writeln!(out, "## Observability diagnostics\n").unwrap();
    if summary.diagnostics.is_empty() {
        writeln!(out, "_None._").unwrap();
        return;
    }
    writeln!(out, "| Severity | Code | Sequence | Detail |").unwrap();
    writeln!(out, "| --- | --- | ---: | --- |").unwrap();
    for diagnostic in &summary.diagnostics {
        writeln!(
            out,
            "| {} | {} | {} | {} |",
            enum_label(&diagnostic.severity),
            enum_label(&diagnostic.code),
            diagnostic
                .seq
                .map(|seq| seq.to_string())
                .unwrap_or_else(|| "—".to_string()),
            escape_cell(&diagnostic.message),
        )
        .unwrap();
    }
}

fn row(out: &mut String, label: &str, value: impl std::fmt::Display) {
    writeln!(out, "| {} | {} |", escape_cell(label), value).unwrap();
}

fn coverage(value: &MetricCoverageV1) -> String {
    match value.expected {
        Some(expected) => format!("{}/{expected}", value.observed),
        None => format!("{}/?", value.observed),
    }
}

fn optional_ms(value: Option<u64>) -> String {
    value
        .map(|value| format!("{value} ms"))
        .unwrap_or_else(unavailable)
}

fn optional_count(value: Option<u64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(unavailable)
}

fn unavailable() -> String {
    "unavailable".to_string()
}

fn enum_label(value: &impl Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

fn code(value: &str) -> String {
    format!("`{}`", escape_cell(&value.replace('`', "\\`")))
}

fn escape_cell(value: &str) -> String {
    value
        .replace('|', "\\|")
        .replace(['\r', '\n'], " ")
        .trim()
        .to_string()
}
