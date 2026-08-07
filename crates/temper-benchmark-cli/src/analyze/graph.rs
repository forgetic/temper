// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeMap;

use temper_protocol_activity::{AgentActivityEventV1, ToolFailureCategoryV1, ToolStatusV1};

use super::{CallKey, captured_command, content_text, shell::classify_shell_discovery};
use crate::{
    AnalyzeOptions, ConventionalDiscoveryMetricsV1, DiagnosticSeverityV1, GraphMetricsV1,
    MetricCoverageV1, NormalizedTrace, TraceDiagnosticCodeV1, TraceDiagnosticV1,
};

mod relevance;

use relevance::classify_relevance;

pub(super) struct GraphAnalysis {
    pub metrics: Option<GraphMetricsV1>,
    pub diagnostics: Vec<TraceDiagnosticV1>,
}

#[derive(Default)]
struct GraphCall {
    call_id: String,
    scope_id: String,
    name: String,
    start_seq: Option<u64>,
    finish_seq: Option<u64>,
    arguments: Option<String>,
    status: Option<ToolStatusV1>,
    result: Option<String>,
    failure: Option<ToolFailureCategoryV1>,
    readiness_wait_ms: Option<u64>,
    discovery_duration_ms: Option<u64>,
}

#[derive(Default)]
struct Action {
    call_id: String,
    scope_id: String,
    name: String,
    start_seq: u64,
    finish_seq: Option<u64>,
    arguments: Option<String>,
    status: Option<ToolStatusV1>,
}

pub(super) fn graph_metrics(trace: &NormalizedTrace, options: &AnalyzeOptions) -> GraphAnalysis {
    let calls = collect_graph_calls(trace);
    if calls.is_empty() && options.graph_decision_targets.is_empty() {
        return GraphAnalysis {
            metrics: None,
            diagnostics: Vec::new(),
        };
    }
    let mut diagnostics = Vec::new();
    let call_count = calls.len() as u64;
    let succeeded = calls
        .values()
        .filter(|call| call.status == Some(ToolStatusV1::Succeeded))
        .count() as u64;
    let failed = calls
        .values()
        .filter(|call| call.status == Some(ToolStatusV1::Failed))
        .count() as u64;
    let cancelled = calls
        .values()
        .filter(|call| call.status == Some(ToolStatusV1::Cancelled))
        .count() as u64;
    let status_observed = succeeded.saturating_add(failed).saturating_add(cancelled);

    let non_success = failed.saturating_add(cancelled);
    let mut failures_by_category = BTreeMap::new();
    let mut failure_observed = 0_u64;
    for call in calls.values().filter(|call| {
        matches!(
            call.status,
            Some(ToolStatusV1::Failed | ToolStatusV1::Cancelled)
        )
    }) {
        if let Some(category) = call.failure {
            *failures_by_category.entry(category).or_default() += 1;
            failure_observed = failure_observed.saturating_add(1);
        }
    }

    let (readiness_wait, readiness_observed) = timing_total(
        calls.values().map(|call| call.readiness_wait_ms),
        call_count,
    );
    let (discovery_duration, discovery_observed) = timing_total(
        calls.values().map(|call| call.discovery_duration_ms),
        call_count,
    );
    if readiness_observed < call_count || discovery_observed < call_count {
        diagnostics.push(unavailable(
            None,
            "one or more graph calls lack codebase-memory timing components",
        ));
    }

    let immediate_repeats = immediate_repeats(trace, &calls);
    let immediate_repeat = (failure_observed == non_success).then_some(immediate_repeats);

    let actions = collect_actions(trace);
    let relevance = classify_relevance(options, &calls, &actions);
    diagnostics.extend(relevance.diagnostics);
    let decision = decisive_selection(options, &actions);
    diagnostics.extend(decision.diagnostics);
    let conventional = decision
        .sequence
        .map(|sequence| conventional_discovery(trace, options, sequence));
    let conventional = conventional.map(|analysis| {
        diagnostics.extend(analysis.diagnostics);
        analysis.metrics
    });

    GraphAnalysis {
        metrics: Some(GraphMetricsV1 {
            calls: call_count,
            succeeded,
            failed,
            cancelled,
            failures_by_category,
            status_coverage: MetricCoverageV1 {
                observed: status_observed,
                expected: Some(call_count),
            },
            failure_category_coverage: MetricCoverageV1 {
                observed: failure_observed,
                expected: Some(non_success),
            },
            cumulative_readiness_wait_ms: readiness_wait,
            readiness_wait_coverage: MetricCoverageV1 {
                observed: readiness_observed,
                expected: Some(call_count),
            },
            cumulative_discovery_duration_ms: discovery_duration,
            discovery_duration_coverage: MetricCoverageV1 {
                observed: discovery_observed,
                expected: Some(call_count),
            },
            immediate_repeated_attempts_after_systemic_failure: immediate_repeat,
            immediate_repeat_coverage: MetricCoverageV1 {
                observed: failure_observed,
                expected: Some(non_success),
            },
            relevant_results: relevance.relevant,
            irrelevant_successes: relevance.irrelevant,
            relevance_coverage: MetricCoverageV1 {
                observed: relevance.observed,
                expected: Some(succeeded),
            },
            decision_evidence: relevance.evidence,
            conventional_discovery_before_selection: conventional,
        }),
        diagnostics,
    }
}

fn collect_graph_calls(trace: &NormalizedTrace) -> BTreeMap<CallKey, GraphCall> {
    let mut calls = BTreeMap::new();
    for event in &trace.events {
        match &event.event {
            AgentActivityEventV1::ToolStarted(tool) if is_graph_tool(&tool.name) => {
                calls
                    .entry(CallKey::new(event, &tool.call_id))
                    .or_insert_with(|| GraphCall {
                        call_id: tool.call_id.clone(),
                        scope_id: event.scope.id.clone(),
                        name: tool.name.clone(),
                        start_seq: Some(event.seq),
                        arguments: tool
                            .arguments
                            .as_ref()
                            .and_then(|arguments| content_text(trace, arguments)),
                        ..GraphCall::default()
                    });
            }
            AgentActivityEventV1::ToolFinished(tool) if is_graph_tool(&tool.name) => {
                let call = calls
                    .entry(CallKey::new(event, &tool.call_id))
                    .or_insert_with(|| GraphCall {
                        call_id: tool.call_id.clone(),
                        scope_id: event.scope.id.clone(),
                        name: tool.name.clone(),
                        ..GraphCall::default()
                    });
                call.finish_seq = Some(event.seq);
                call.status = Some(tool.status);
                call.failure = tool.failure.as_ref().map(|failure| failure.category);
                if tool.status == ToolStatusV1::Succeeded {
                    call.result = tool
                        .result
                        .as_ref()
                        .and_then(|result| content_text(trace, result));
                }
                if let Some(timing) = tool.codebase_memory_timing {
                    call.readiness_wait_ms = Some(timing.readiness_wait_ms);
                    call.discovery_duration_ms = Some(timing.graph_execution_ms);
                }
            }
            _ => {}
        }
    }
    calls
}

fn collect_actions(trace: &NormalizedTrace) -> Vec<Action> {
    let mut actions = trace
        .events
        .iter()
        .filter_map(|event| match &event.event {
            AgentActivityEventV1::ToolStarted(tool) if is_action_tool(&tool.name) => Some((
                CallKey::new(event, &tool.call_id),
                Action {
                    call_id: tool.call_id.clone(),
                    scope_id: event.scope.id.clone(),
                    name: tool.name.clone(),
                    start_seq: event.seq,
                    finish_seq: None,
                    arguments: tool
                        .arguments
                        .as_ref()
                        .and_then(|arguments| content_text(trace, arguments)),
                    status: None,
                },
            )),
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    for event in &trace.events {
        let AgentActivityEventV1::ToolFinished(tool) = &event.event else {
            continue;
        };
        if !is_action_tool(&tool.name) {
            continue;
        }
        if let Some(action) = actions.get_mut(&CallKey::new(event, &tool.call_id)) {
            action.finish_seq = Some(event.seq);
            action.status = Some(tool.status);
        }
    }
    actions
        .into_values()
        .filter(|action| action.status == Some(ToolStatusV1::Succeeded))
        .collect()
}

fn timing_total(values: impl Iterator<Item = Option<u64>>, expected: u64) -> (Option<u64>, u64) {
    let mut total = 0_u64;
    let mut observed = 0_u64;
    for value in values.flatten() {
        total = total.saturating_add(value);
        observed = observed.saturating_add(1);
    }
    ((observed > 0 || expected == 0).then_some(total), observed)
}

fn immediate_repeats(trace: &NormalizedTrace, calls: &BTreeMap<CallKey, GraphCall>) -> u64 {
    let starts = trace
        .events
        .iter()
        .filter(|event| matches!(event.event, AgentActivityEventV1::ToolStarted(_)))
        .collect::<Vec<_>>();
    calls
        .values()
        .filter(|call| call.failure.is_some_and(systemic_failure))
        .filter(|call| {
            let Some(finish_seq) = call.finish_seq else {
                return false;
            };
            starts
                .iter()
                .find(|event| event.scope.id == call.scope_id && event.seq > finish_seq)
                .is_some_and(|event| {
                    matches!(
                        &event.event,
                        AgentActivityEventV1::ToolStarted(tool) if is_graph_tool(&tool.name)
                    )
                })
        })
        .count() as u64
}

fn systemic_failure(category: ToolFailureCategoryV1) -> bool {
    category != ToolFailureCategoryV1::InvalidModelInput
}

struct DecisiveSelection {
    sequence: Option<u64>,
    diagnostics: Vec<TraceDiagnosticV1>,
}

/// Finds the first fixture-declared target selected by a successful read or
/// mutation. This boundary is intentionally independent of graph relevance:
/// graph-disabled and graph-unavailable runs still need comparable fallback
/// discovery measurements.
fn decisive_selection(options: &AnalyzeOptions, actions: &[Action]) -> DecisiveSelection {
    if options.graph_decision_targets.is_empty() {
        return DecisiveSelection {
            sequence: None,
            diagnostics: Vec::new(),
        };
    }

    let sequence = actions
        .iter()
        .filter(|action| is_selection_tool(&action.name))
        .filter(|action| {
            action.arguments.as_deref().is_some_and(|arguments| {
                options
                    .graph_decision_targets
                    .iter()
                    .any(|target| selection_matches_target(arguments, &target.target))
            })
        })
        .map(|action| action.start_seq)
        .min();
    let unknown_before_boundary = actions.iter().any(|action| {
        is_selection_tool(&action.name)
            && action.arguments.is_none()
            && sequence.is_none_or(|sequence| action.start_seq < sequence)
    });
    if unknown_before_boundary {
        return DecisiveSelection {
            sequence: None,
            diagnostics: vec![unavailable(
                sequence,
                "a successful selection omits arguments needed to locate the decisive selection boundary",
            )],
        };
    }
    if sequence.is_none() {
        return DecisiveSelection {
            sequence: None,
            diagnostics: vec![unavailable(
                None,
                "no successful selection matched a declared graph decision target",
            )],
        };
    }
    DecisiveSelection {
        sequence,
        diagnostics: Vec::new(),
    }
}

fn conventional_discovery(
    trace: &NormalizedTrace,
    options: &AnalyzeOptions,
    before_seq: u64,
) -> ConventionalDiscoveryAnalysis {
    // This intentionally recognizes only an unquoted, unescaped shell list
    // joined by `&&`, `||`, `;`, or newlines. Treating richer shell syntax as
    // unknown avoids counting a word inside a quote, expansion, pipeline, or
    // redirection as a separate conventional-discovery command.
    let mut grep_calls = 0_u64;
    let mut find_calls = 0_u64;
    let mut read_calls = 0_u64;
    let mut shell_segments = 0_u64;
    let mut shell_observed = 0_u64;
    let mut shell_expected = 0_u64;
    let mut diagnostics = Vec::new();
    for event in trace.events.iter().filter(|event| event.seq < before_seq) {
        let AgentActivityEventV1::ToolStarted(tool) = &event.event else {
            continue;
        };
        match tool.name.as_str() {
            "grep" => grep_calls = grep_calls.saturating_add(1),
            "find" => find_calls = find_calls.saturating_add(1),
            "read" => read_calls = read_calls.saturating_add(1),
            "bash" => {
                shell_expected = shell_expected.saturating_add(1);
                let command = tool
                    .arguments
                    .as_ref()
                    .and_then(|arguments| content_text(trace, arguments))
                    .and_then(|arguments| captured_command(&arguments));
                match command.as_deref().map(|command| {
                    classify_shell_discovery(command, &options.discovery_command_prefixes)
                }) {
                    Some(Ok(classified_segments)) => {
                        shell_observed = shell_observed.saturating_add(1);
                        shell_segments = shell_segments.saturating_add(classified_segments);
                    }
                    Some(Err(error)) => diagnostics.push(unavailable(
                        Some(event.seq),
                        error.availability_message(),
                    )),
                    None => diagnostics.push(unavailable(
                        Some(event.seq),
                        "shell discovery classification is unavailable because the complete command is omitted or truncated",
                    )),
                }
            }
            _ => {}
        }
    }
    let known_total = grep_calls
        .saturating_add(find_calls)
        .saturating_add(read_calls)
        .saturating_add(shell_segments);
    ConventionalDiscoveryAnalysis {
        metrics: ConventionalDiscoveryMetricsV1 {
            grep_calls,
            find_calls,
            read_calls,
            classified_shell_segments: shell_segments,
            total_calls: (shell_observed == shell_expected).then_some(known_total),
            shell_command_classification_coverage: MetricCoverageV1 {
                observed: shell_observed,
                expected: Some(shell_expected),
            },
        },
        diagnostics,
    }
}

struct ConventionalDiscoveryAnalysis {
    metrics: ConventionalDiscoveryMetricsV1,
    diagnostics: Vec<TraceDiagnosticV1>,
}

fn selection_matches_target(arguments: &str, target: &str) -> bool {
    arguments.trim() == target.trim()
}

fn is_graph_tool(name: &str) -> bool {
    name.starts_with("codebase_memory_")
}

fn is_selection_tool(name: &str) -> bool {
    matches!(name, "read" | "edit" | "write")
}

fn is_action_tool(name: &str) -> bool {
    is_selection_tool(name) || name == "apply_patch"
}

fn unavailable(seq: Option<u64>, message: &str) -> TraceDiagnosticV1 {
    TraceDiagnosticV1 {
        code: TraceDiagnosticCodeV1::GraphEvidenceUnavailable,
        severity: DiagnosticSeverityV1::Warning,
        message: message.to_string(),
        seq,
    }
}
