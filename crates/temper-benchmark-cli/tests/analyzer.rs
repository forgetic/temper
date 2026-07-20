// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use temper_benchmark_cli::{
    AnalyzeOptions, MetricCoverageV1, RunTerminalStatusV1, TraceDiagnosticCodeV1, analyze_trace,
    ingest_trace, render_run_summary_json, render_run_summary_markdown,
};
use temper_protocol_activity::{
    AgentActivityEventV1, CaptureModeV1, FailureCodeV1, FailureInfoV1, RunFailedV1,
};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

#[test]
fn analyzer_derives_retries_ttft_tokens_tools_and_structure() {
    let trace = ingest_trace(fixture("metrics-events.jsonl")).unwrap();
    let summary = analyze_trace(
        &trace,
        &AnalyzeOptions {
            validation_command_prefixes: vec!["cargo test".to_string()],
        },
    );

    assert_eq!(summary.wall_time_ms, Some(1_000));
    let terminal = summary.terminal.as_ref().unwrap();
    assert_eq!(terminal.status, RunTerminalStatusV1::Cancelled);
    assert_eq!(summary.metrics.turns, Some(2));

    let model = summary.metrics.model.as_ref().unwrap();
    assert_eq!(model.calls, 2);
    assert_eq!(model.attempts, 3);
    assert_eq!(model.succeeded_attempts, 1);
    assert_eq!(model.failed_attempts, 1);
    assert_eq!(model.cancelled_attempts, 1);
    assert_eq!(model.retries, 1);
    assert_eq!(model.provider_failures, 1);
    assert_eq!(model.cumulative_duration_ms, Some(370));
    assert_eq!(
        model.duration_coverage,
        MetricCoverageV1 {
            observed: 3,
            expected: Some(3),
        }
    );
    assert_eq!(model.cumulative_time_to_first_token_ms, Some(50));
    assert_eq!(
        model.time_to_first_token_coverage,
        MetricCoverageV1 {
            observed: 1,
            expected: Some(3),
        }
    );

    let tokens = summary.metrics.tokens.as_ref().unwrap();
    assert_eq!(tokens.input_tokens, 100);
    assert_eq!(tokens.output_tokens, 20);
    assert_eq!(tokens.cache_read_tokens, 30);
    assert_eq!(tokens.cache_write_tokens, 4);
    assert_eq!(tokens.coverage.observed, 1);
    assert_eq!(tokens.coverage.expected, Some(1));

    let tools = summary.metrics.tools.as_ref().unwrap();
    assert_eq!(tools.calls, 10);
    assert_eq!((tools.succeeded, tools.failed, tools.cancelled), (8, 1, 1));
    assert_eq!(tools.cumulative_duration_ms, Some(323));
    assert_eq!(tools.by_name["edit"].calls, 2);
    assert_eq!(tools.by_name["edit"].failed, 1);
    assert_eq!(tools.by_name["submit_for_pr"].calls, 3);
    assert_eq!(
        tools
            .slowest
            .iter()
            .take(4)
            .map(|call| (call.name.as_str(), call.call_id.as_str(), call.duration_ms))
            .collect::<Vec<_>>(),
        vec![
            ("bash", "validate-1", 150),
            ("grep", "grep-1", 40),
            ("read", "read-1", 40),
            ("write", "write-1", 40),
        ]
    );

    let structure = summary.metrics.structure.as_ref().unwrap();
    assert_eq!(structure.failed_edit_attempts, Some(1));
    assert_eq!(structure.mutations, Some(3));
    assert_eq!(structure.validation_boundaries, Some(3));
    assert_eq!(structure.post_validation_mutations, Some(2));
    assert_eq!(structure.validation_invalidations, Some(2));
    assert_eq!(structure.revalidations, Some(2));
}

#[test]
fn omitted_validation_content_makes_ordering_metrics_unavailable() {
    let mut trace = ingest_trace(fixture("metrics-events.jsonl")).unwrap();
    for event in &mut trace.events {
        match &mut event.event {
            AgentActivityEventV1::RunStarted(started) => started.capture = CaptureModeV1::Metadata,
            AgentActivityEventV1::ToolStarted(tool) if tool.name == "bash" => {
                tool.arguments = None;
            }
            AgentActivityEventV1::ToolFinished(tool) if tool.name == "submit_for_pr" => {
                tool.result = None;
            }
            _ => {}
        }
    }

    let summary = analyze_trace(
        &trace,
        &AnalyzeOptions {
            validation_command_prefixes: vec!["cargo test".to_string()],
        },
    );
    let structure = summary.metrics.structure.as_ref().unwrap();
    assert_eq!(structure.failed_edit_attempts, Some(1));
    assert_eq!(structure.mutations, Some(3));
    assert_eq!(structure.validation_boundaries, None);
    assert_eq!(structure.post_validation_mutations, None);
    assert_eq!(structure.validation_invalidations, None);
    assert_eq!(structure.revalidations, None);
    assert_eq!(
        summary
            .diagnostics
            .iter()
            .filter(|diagnostic| {
                diagnostic.code == TraceDiagnosticCodeV1::StructureEvidenceUnavailable
            })
            .count(),
        4
    );
}

#[test]
fn incomplete_calls_keep_counts_and_expose_duration_coverage() {
    let mut trace = ingest_trace(fixture("metrics-events.jsonl")).unwrap();
    trace.events.retain(|event| {
        !matches!(
            &event.event,
            AgentActivityEventV1::ModelCallFinished(call) if call.call_id == "model-2"
        ) && !matches!(
            &event.event,
            AgentActivityEventV1::ToolFinished(call) if call.call_id == "read-1"
        )
    });

    let summary = trace.run_summary();
    let model = summary.metrics.model.as_ref().unwrap();
    assert_eq!(model.calls, 2);
    assert_eq!(model.attempts, 3);
    assert_eq!(model.duration_coverage.observed, 2);
    assert_eq!(model.duration_coverage.expected, Some(3));
    let tools = summary.metrics.tools.as_ref().unwrap();
    assert_eq!(tools.calls, 10);
    assert_eq!(tools.duration_coverage.observed, 9);
    assert_eq!(tools.duration_coverage.expected, Some(10));
    assert_eq!(tools.by_name["read"].cumulative_duration_ms, None);
    assert!(!tools.slowest.iter().any(|call| call.call_id == "read-1"));
}

#[test]
fn failed_terminal_uses_typed_reason_and_elapsed_wall_time() {
    let mut trace = ingest_trace(fixture("metrics-events.jsonl")).unwrap();
    let terminal = trace.events.last_mut().unwrap();
    terminal.event = AgentActivityEventV1::RunFailed(RunFailedV1 {
        failure: FailureInfoV1 {
            code: FailureCodeV1::Timeout,
            message: "agent deadline exceeded".to_string(),
            retryable: false,
        },
    });

    let summary = trace.run_summary();
    let terminal = summary.terminal.as_ref().unwrap();
    assert_eq!(terminal.status, RunTerminalStatusV1::Failed);
    assert_eq!(terminal.duration_ms, Some(1_000));
    assert_eq!(
        terminal.failure.as_ref().unwrap().code,
        FailureCodeV1::Timeout
    );
    assert_eq!(summary.wall_time_ms, Some(1_000));
}

#[test]
fn run_summary_json_and_markdown_match_goldens() {
    let trace = ingest_trace(fixture("metrics-events.jsonl")).unwrap();
    let summary = trace.run_summary();

    assert_eq!(
        render_run_summary_json(&summary).unwrap(),
        include_str!("../fixtures/metrics-run.json")
    );
    assert_eq!(
        render_run_summary_markdown(&summary),
        include_str!("../fixtures/metrics-run.md")
    );
}

#[test]
fn analyze_cli_writes_stable_artifacts_and_canonical_trace() {
    let temporary = tempfile::tempdir().unwrap();
    let output_dir = temporary.path().join("analysis");
    let trace_path = fixture("metrics-events.jsonl");
    let output = Command::new(env!("CARGO_BIN_EXE_temper-benchmark"))
        .args([
            "analyze",
            "--output-dir",
            output_dir.to_str().unwrap(),
            "--trace",
            trace_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        include_str!("../fixtures/metrics-run.md")
    );
    assert_eq!(
        fs::read_to_string(output_dir.join("run.json")).unwrap(),
        include_str!("../fixtures/metrics-run.json")
    );
    assert_eq!(
        fs::read_to_string(output_dir.join("run.md")).unwrap(),
        include_str!("../fixtures/metrics-run.md")
    );
    let canonical = ingest_trace(output_dir.join("trace.export.jsonl")).unwrap();
    assert_eq!(canonical.events, ingest_trace(trace_path).unwrap().events);
}

#[test]
fn analyze_cli_rejects_missing_duplicate_and_unknown_arguments() {
    for args in [
        vec!["analyze", "--trace", "trace.jsonl"],
        vec![
            "analyze",
            "--trace",
            "one.jsonl",
            "--trace",
            "two.jsonl",
            "--output-dir",
            "out",
        ],
        vec![
            "analyze",
            "--trace",
            "trace.jsonl",
            "--output-dir",
            "out",
            "--surprise",
            "yes",
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_temper-benchmark"))
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(64));
        assert!(String::from_utf8_lossy(&output.stderr).contains("Usage:"));
    }
}
