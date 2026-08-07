// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use temper_benchmark_cli::{
    AnalyzeOptions, GraphDecisionKindV1, GraphDecisionTargetV1, MetricCoverageV1,
    RunTerminalStatusV1, TraceDiagnosticCodeV1, analyze_trace, ingest_trace,
    render_run_summary_json, render_run_summary_markdown,
};
use temper_protocol_activity::{
    AgentActivityEventV1, CaptureModeV1, FailureCodeV1, FailureInfoV1, RunFailedV1, ToolStatusV1,
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
            ..AnalyzeOptions::default()
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
    assert_eq!(structure.mutation_turns, Some(1));
    assert_eq!(structure.single_mutation_turns, Some(0));
    assert_eq!(structure.max_mutations_per_turn, Some(3));
    assert_eq!(structure.validation_boundaries, Some(3));
    assert_eq!(structure.post_validation_mutations, Some(2));
    assert_eq!(structure.validation_invalidations, Some(2));
    assert_eq!(structure.revalidations, Some(2));
}

#[test]
fn mutation_turn_metrics_count_one_mutation_per_turn() {
    let mut trace = ingest_trace(fixture("metrics-events.jsonl")).unwrap();
    let mut next_turn = 0;
    for event in &mut trace.events {
        if matches!(
            &event.event,
            AgentActivityEventV1::ToolFinished(tool)
                if matches!(tool.name.as_str(), "write" | "edit")
                    && tool.status == ToolStatusV1::Succeeded
        ) {
            event.turn = Some(next_turn);
            next_turn += 1;
        }
    }

    let summary = analyze_trace(&trace, &AnalyzeOptions::default());
    let structure = summary.metrics.structure.as_ref().unwrap();
    assert_eq!(structure.mutations, Some(3));
    assert_eq!(structure.mutation_turns, Some(3));
    assert_eq!(structure.single_mutation_turns, Some(3));
    assert_eq!(structure.max_mutations_per_turn, Some(1));
}

#[test]
fn mutation_turn_metrics_keep_equal_turn_numbers_in_separate_scopes() {
    let mut trace = ingest_trace(fixture("parallel-child-scopes.jsonl")).unwrap();
    for event in &mut trace.events {
        if event.scope.id != "child-a" {
            continue;
        }
        match &mut event.event {
            AgentActivityEventV1::ToolStarted(tool) if tool.name == "read" => {
                tool.name = "write".to_string();
            }
            AgentActivityEventV1::ToolFinished(tool) if tool.name == "read" => {
                tool.name = "write".to_string();
            }
            _ => {}
        }
    }

    let summary = analyze_trace(&trace, &AnalyzeOptions::default());
    let structure = summary.metrics.structure.as_ref().unwrap();
    assert_eq!(structure.mutations, Some(2));
    assert_eq!(structure.mutation_turns, Some(2));
    assert_eq!(structure.single_mutation_turns, Some(2));
    assert_eq!(structure.max_mutations_per_turn, Some(1));
}

#[test]
fn missing_historical_mutation_turn_makes_batching_metrics_unavailable() {
    let mut trace = ingest_trace(fixture("metrics-events.jsonl")).unwrap();
    let mutation = trace
        .events
        .iter_mut()
        .find(|event| {
            matches!(
                &event.event,
                AgentActivityEventV1::ToolFinished(tool)
                    if matches!(tool.name.as_str(), "write" | "edit")
                        && tool.status == ToolStatusV1::Succeeded
            )
        })
        .unwrap();
    mutation.turn = None;
    let missing_seq = mutation.seq;

    let summary = analyze_trace(&trace, &AnalyzeOptions::default());
    let structure = summary.metrics.structure.as_ref().unwrap();
    assert_eq!(structure.mutations, Some(3));
    assert_eq!(structure.mutation_turns, None);
    assert_eq!(structure.single_mutation_turns, None);
    assert_eq!(structure.max_mutations_per_turn, None);
    assert!(summary.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == TraceDiagnosticCodeV1::StructureEvidenceUnavailable
            && diagnostic.seq == Some(missing_seq)
            && diagnostic.message.contains("lacks turn identity")
    }));
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
            ..AnalyzeOptions::default()
        },
    );
    let structure = summary.metrics.structure.as_ref().unwrap();
    assert_eq!(structure.failed_edit_attempts, Some(1));
    assert_eq!(structure.mutations, Some(3));
    assert_eq!(structure.mutation_turns, Some(1));
    assert_eq!(structure.single_mutation_turns, Some(0));
    assert_eq!(structure.max_mutations_per_turn, Some(3));
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
fn graph_metrics_distinguish_consumption_failures_retries_and_fallback_discovery() {
    let trace = ingest_trace(fixture("graph-metrics-events.jsonl")).unwrap();
    let summary = analyze_trace(
        &trace,
        &AnalyzeOptions {
            discovery_command_prefixes: vec![vec!["git".to_string(), "grep".to_string()]],
            graph_decision_targets: vec![
                GraphDecisionTargetV1 {
                    target: "src/lib.rs".to_string(),
                    kind: GraphDecisionKindV1::Implementation,
                    result_contains: None,
                    consumption: Vec::new(),
                },
                GraphDecisionTargetV1 {
                    target: "src/main.rs".to_string(),
                    kind: GraphDecisionKindV1::Caller,
                    result_contains: None,
                    consumption: Vec::new(),
                },
                GraphDecisionTargetV1 {
                    target: "tests/focused.rs".to_string(),
                    kind: GraphDecisionKindV1::FocusedTest,
                    result_contains: None,
                    consumption: Vec::new(),
                },
            ],
            ..AnalyzeOptions::default()
        },
    );

    let graph = summary.metrics.graph.as_ref().unwrap();
    assert_eq!(
        (graph.calls, graph.succeeded, graph.failed, graph.cancelled),
        (7, 4, 3, 0)
    );
    assert_eq!(
        graph.status_coverage,
        MetricCoverageV1 {
            observed: 7,
            expected: Some(7)
        }
    );
    assert_eq!(
        graph.failures_by_category[&temper_protocol_activity::ToolFailureCategoryV1::Timeout],
        1
    );
    assert_eq!(
        graph.failures_by_category[&temper_protocol_activity::ToolFailureCategoryV1::IndexFailure],
        1
    );
    assert_eq!(
        graph.failures_by_category[&temper_protocol_activity::ToolFailureCategoryV1::CircuitOpen],
        1
    );
    assert_eq!(
        graph.failure_category_coverage,
        MetricCoverageV1 {
            observed: 3,
            expected: Some(3)
        }
    );
    assert_eq!(graph.cumulative_readiness_wait_ms, Some(44));
    assert_eq!(graph.cumulative_discovery_duration_ms, Some(197));
    assert_eq!(graph.readiness_wait_coverage.observed, 7);
    assert_eq!(
        graph.immediate_repeated_attempts_after_systemic_failure,
        Some(1)
    );
    assert_eq!(graph.relevant_results, Some(3));
    assert_eq!(graph.irrelevant_successes, Some(1));
    assert_eq!(
        graph.relevance_coverage,
        MetricCoverageV1 {
            observed: 4,
            expected: Some(4)
        }
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
    assert_eq!(discovery.classified_shell_segments, 1);
    assert_eq!(discovery.total_calls, Some(4));

    let markdown = render_run_summary_markdown(&summary);
    assert!(markdown.contains("## Graph discovery and decision relevance"));
    assert!(markdown.contains("| Relevant graph results | 3 |"));
    assert!(markdown.contains("| Task correctness | unavailable |"));
    assert!(markdown.contains("| Host validation | unavailable |"));
}

#[test]
fn decisive_selection_keeps_disabled_and_irrelevant_graph_fallback_comparable() {
    let target = GraphDecisionTargetV1 {
        target: "src/lib.rs".to_string(),
        kind: GraphDecisionKindV1::Implementation,
        result_contains: None,
        consumption: Vec::new(),
    };
    let options = AnalyzeOptions {
        discovery_command_prefixes: vec![vec!["git".to_string(), "grep".to_string()]],
        graph_decision_targets: vec![target],
        ..AnalyzeOptions::default()
    };

    let mut disabled = ingest_trace(fixture("graph-metrics-events.jsonl")).unwrap();
    disabled.events.retain(|event| {
        !matches!(
            &event.event,
            AgentActivityEventV1::ToolStarted(tool) if tool.name.starts_with("codebase_memory_")
        ) && !matches!(
            &event.event,
            AgentActivityEventV1::ToolFinished(tool) if tool.name.starts_with("codebase_memory_")
        )
    });
    let disabled = analyze_trace(&disabled, &options);
    let graph = disabled.metrics.graph.as_ref().unwrap();
    assert_eq!((graph.calls, graph.succeeded, graph.failed), (0, 0, 0));
    assert_eq!(graph.relevant_results, Some(0));
    assert_eq!(graph.irrelevant_successes, Some(0));
    assert_eq!(
        graph
            .conventional_discovery_before_selection
            .as_ref()
            .unwrap()
            .total_calls,
        Some(4)
    );

    let mut irrelevant = ingest_trace(fixture("graph-metrics-events.jsonl")).unwrap();
    for event in &mut irrelevant.events {
        if let AgentActivityEventV1::ToolFinished(tool) = &mut event.event {
            if tool.call_id == "graph-implementation" {
                let Some(temper_protocol_activity::CapturedContentV1::Inline(result)) =
                    tool.result.as_mut()
                else {
                    panic!("fixture graph result must be inline");
                };
                result.text = "unrelated docs/design.md".to_string();
            }
        }
    }
    let irrelevant = analyze_trace(&irrelevant, &options);
    let graph = irrelevant.metrics.graph.as_ref().unwrap();
    assert_eq!(graph.relevant_results, Some(0));
    assert_eq!(graph.irrelevant_successes, Some(4));
    assert_eq!(
        graph
            .conventional_discovery_before_selection
            .as_ref()
            .unwrap()
            .total_calls,
        Some(4)
    );
}

#[test]
fn partial_graph_timing_keeps_coverage_and_partial_total() {
    let mut trace = ingest_trace(fixture("graph-metrics-events.jsonl")).unwrap();
    let event = trace
        .events
        .iter_mut()
        .find(|event| {
            matches!(
                &event.event,
                AgentActivityEventV1::ToolFinished(tool) if tool.call_id == "graph-caller"
            )
        })
        .unwrap();
    let AgentActivityEventV1::ToolFinished(tool) = &mut event.event else {
        unreachable!();
    };
    tool.codebase_memory_timing = None;
    let summary = analyze_trace(&trace, &AnalyzeOptions::default());
    let graph = summary.metrics.graph.as_ref().unwrap();
    assert_eq!(graph.readiness_wait_coverage.observed, 6);
    assert_eq!(graph.readiness_wait_coverage.expected, Some(7));
    assert_eq!(graph.cumulative_readiness_wait_ms, Some(40));
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
                result_contains: None,
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
            && diagnostic.message.contains("omitted or truncated")
    }));
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
