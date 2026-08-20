// SPDX-License-Identifier: MPL-2.0

//! Typed metric extraction over a normalized activity stream.

mod graph;
mod ordinary;
mod shell;

use std::collections::{BTreeMap, BTreeSet};

use temper_protocol_activity::{
    AgentActivityEventV1, AgentRunEventV1, CapturedContentV1, FailureCodeV1, ModelCallStatusV1,
    ToolStatusV1,
};

use crate::{
    DiagnosticSeverityV1, GraphDecisionTargetV1, MetricCoverageV1, ModelMetricsV1, NormalizedTrace,
    RunSummaryV1, SlowToolCallV1, StructureMetricsV1, TokenMetricsV1, ToolMetricsV1,
    ToolNameMetricsV1, TraceDiagnosticCodeV1, TraceDiagnosticV1,
};

use graph::graph_metrics;
use ordinary::ordinary_tool_metrics;

/// Analyzer settings supplied by a benchmark manifest or an operator.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AnalyzeOptions {
    /// Successful `bash` calls whose captured command starts with one of these
    /// values are validation boundaries.
    pub validation_command_prefixes: Vec<String>,
    /// `bash` argv prefixes explicitly classified as conventional discovery.
    pub discovery_command_prefixes: Vec<Vec<String>>,
    /// Fixture-owned decision targets used by the graph-consumption rubric.
    pub graph_decision_targets: Vec<GraphDecisionTargetV1>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct CallKey {
    scope_id: String,
    call_id: String,
}

impl CallKey {
    fn new(event: &AgentRunEventV1, call_id: &str) -> Self {
        Self {
            scope_id: event.scope.id.clone(),
            call_id: call_id.to_string(),
        }
    }
}

/// Derives all metrics observable from the normalized typed event stream.
pub fn analyze_trace(trace: &NormalizedTrace, options: &AnalyzeOptions) -> RunSummaryV1 {
    let mut summary = trace.base_run_summary();
    let turns = collect_turns(&trace.events);
    summary.metrics.turns = Some(turns.len() as u64);
    let model = model_metrics(&trace.events);
    summary.metrics.tokens = Some(token_metrics(&trace.events, model.succeeded_attempts));
    summary.metrics.model = Some(model);
    let tools = tool_metrics(&trace.events);
    summary.metrics.tools = Some(tools.metrics);
    summary.diagnostics.extend(tools.diagnostics);
    let graph = graph_metrics(trace, options);
    summary.metrics.graph = graph.metrics;
    summary.diagnostics.extend(graph.diagnostics);

    let structure = structure_metrics(trace, options);
    summary.metrics.structure = Some(structure.metrics);
    summary.diagnostics.extend(structure.diagnostics);
    summary
}

fn collect_turns(events: &[AgentRunEventV1]) -> BTreeSet<(String, u32)> {
    events
        .iter()
        .filter_map(|event| {
            let turn = event.turn?;
            (!matches!(event.event, AgentActivityEventV1::PromptPrepared(_)))
                .then(|| (event.scope.id.clone(), turn))
        })
        .collect()
}

fn model_metrics(events: &[AgentRunEventV1]) -> ModelMetricsV1 {
    let mut calls = BTreeSet::new();
    let mut attempts = BTreeSet::new();
    let mut finished = 0_u64;
    let mut succeeded = 0_u64;
    let mut failed = 0_u64;
    let mut cancelled = 0_u64;
    let mut retries = 0_u64;
    let mut provider_failures = 0_u64;
    let mut duration = 0_u64;
    let mut first_token = 0_u64;
    let mut first_token_count = 0_u64;

    for event in events {
        match &event.event {
            AgentActivityEventV1::ModelCallStarted(call) => {
                let key = CallKey::new(event, &call.call_id);
                calls.insert(key.clone());
                attempts.insert((key, call.attempt));
            }
            AgentActivityEventV1::ModelCallFinished(call) => {
                let key = CallKey::new(event, &call.call_id);
                calls.insert(key.clone());
                attempts.insert((key, call.attempt));
                finished = finished.saturating_add(1);
                duration = duration.saturating_add(call.duration_ms);
                match call.status {
                    ModelCallStatusV1::Succeeded => succeeded = succeeded.saturating_add(1),
                    ModelCallStatusV1::Failed => failed = failed.saturating_add(1),
                    ModelCallStatusV1::Cancelled => cancelled = cancelled.saturating_add(1),
                }
                if let Some(value) = call.time_to_first_token_ms {
                    first_token = first_token.saturating_add(value);
                    first_token_count = first_token_count.saturating_add(1);
                }
            }
            AgentActivityEventV1::ModelCallRetrying(retry) => {
                calls.insert(CallKey::new(event, &retry.call_id));
                retries = retries.saturating_add(1);
                if retry.failure.code == FailureCodeV1::Provider {
                    provider_failures = provider_failures.saturating_add(1);
                }
            }
            AgentActivityEventV1::RunFailed(failure)
                if failure.failure.code == FailureCodeV1::Provider =>
            {
                provider_failures = provider_failures.saturating_add(1);
            }
            _ => {}
        }
    }

    let attempt_count = attempts.len() as u64;
    ModelMetricsV1 {
        calls: calls.len() as u64,
        attempts: attempt_count,
        succeeded_attempts: succeeded,
        failed_attempts: failed,
        cancelled_attempts: cancelled,
        retries,
        provider_failures,
        cumulative_duration_ms: observed_total(duration, finished, attempt_count),
        duration_coverage: MetricCoverageV1 {
            observed: finished,
            expected: Some(attempt_count),
        },
        cumulative_time_to_first_token_ms: observed_total(
            first_token,
            first_token_count,
            attempt_count,
        ),
        time_to_first_token_coverage: MetricCoverageV1 {
            observed: first_token_count,
            expected: Some(attempt_count),
        },
    }
}

fn token_metrics(events: &[AgentRunEventV1], expected_usage_events: u64) -> TokenMetricsV1 {
    let mut input = 0_u64;
    let mut output = 0_u64;
    let mut cache_read = 0_u64;
    let mut cache_write = 0_u64;
    let mut observed = 0_u64;
    for event in events {
        if let AgentActivityEventV1::Usage(usage) = &event.event {
            input = input.saturating_add(usage.input_tokens);
            output = output.saturating_add(usage.output_tokens);
            cache_read = cache_read.saturating_add(usage.cache_read_tokens);
            cache_write = cache_write.saturating_add(usage.cache_write_tokens);
            observed = observed.saturating_add(1);
        }
    }
    TokenMetricsV1 {
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cache_read,
        cache_write_tokens: cache_write,
        coverage: MetricCoverageV1 {
            observed,
            expected: Some(expected_usage_events),
        },
    }
}

#[derive(Default)]
struct ToolCallMetric {
    name: String,
    status: Option<ToolStatusV1>,
    duration_ms: Option<u64>,
}

struct ToolAnalysis {
    metrics: ToolMetricsV1,
    diagnostics: Vec<TraceDiagnosticV1>,
}

fn tool_metrics(events: &[AgentRunEventV1]) -> ToolAnalysis {
    let mut calls = BTreeMap::<CallKey, ToolCallMetric>::new();
    for event in events {
        match &event.event {
            AgentActivityEventV1::ToolStarted(tool) => {
                calls
                    .entry(CallKey::new(event, &tool.call_id))
                    .or_insert_with(|| ToolCallMetric {
                        name: tool.name.clone(),
                        ..ToolCallMetric::default()
                    });
            }
            AgentActivityEventV1::ToolFinished(tool) => {
                let call = calls.entry(CallKey::new(event, &tool.call_id)).or_default();
                call.name.clone_from(&tool.name);
                call.status = Some(tool.status);
                call.duration_ms = Some(tool.duration_ms);
            }
            _ => {}
        }
    }

    let mut by_name = BTreeMap::<String, ToolNameMetricsV1>::new();
    let mut succeeded = 0_u64;
    let mut failed = 0_u64;
    let mut cancelled = 0_u64;
    let mut duration = 0_u64;
    let mut duration_count = 0_u64;
    let mut slowest = Vec::new();
    for (key, call) in &calls {
        let grouped = by_name
            .entry(call.name.clone())
            .or_insert_with(empty_tool_name_metrics);
        grouped.calls = grouped.calls.saturating_add(1);
        if let Some(status) = call.status {
            increment_status(status, &mut succeeded, &mut failed, &mut cancelled);
            increment_status(
                status,
                &mut grouped.succeeded,
                &mut grouped.failed,
                &mut grouped.cancelled,
            );
        }
        if let Some(value) = call.duration_ms {
            duration = duration.saturating_add(value);
            duration_count = duration_count.saturating_add(1);
            grouped.cumulative_duration_ms = Some(
                grouped
                    .cumulative_duration_ms
                    .unwrap_or_default()
                    .saturating_add(value),
            );
            grouped.duration_coverage.observed =
                grouped.duration_coverage.observed.saturating_add(1);
            slowest.push(SlowToolCallV1 {
                call_id: key.call_id.clone(),
                name: call.name.clone(),
                duration_ms: value,
            });
        }
    }
    for metrics in by_name.values_mut() {
        metrics.duration_coverage.expected = Some(metrics.calls);
        if metrics.calls == 0 {
            metrics.cumulative_duration_ms = Some(0);
        }
    }
    slowest.sort_by(|left, right| {
        right
            .duration_ms
            .cmp(&left.duration_ms)
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.call_id.cmp(&right.call_id))
    });

    let call_count = calls.len() as u64;
    let (ordinary, diagnostics) = ordinary_tool_metrics(events);
    ToolAnalysis {
        metrics: ToolMetricsV1 {
            calls: call_count,
            succeeded,
            failed,
            cancelled,
            cumulative_duration_ms: observed_total(duration, duration_count, call_count),
            duration_coverage: MetricCoverageV1 {
                observed: duration_count,
                expected: Some(call_count),
            },
            by_name,
            slowest,
            ordinary: Some(ordinary),
        },
        diagnostics,
    }
}

fn empty_tool_name_metrics() -> ToolNameMetricsV1 {
    ToolNameMetricsV1 {
        calls: 0,
        succeeded: 0,
        failed: 0,
        cancelled: 0,
        cumulative_duration_ms: None,
        duration_coverage: MetricCoverageV1 {
            observed: 0,
            expected: None,
        },
    }
}

fn increment_status(
    status: ToolStatusV1,
    succeeded: &mut u64,
    failed: &mut u64,
    cancelled: &mut u64,
) {
    match status {
        ToolStatusV1::Succeeded => *succeeded = succeeded.saturating_add(1),
        ToolStatusV1::Failed => *failed = failed.saturating_add(1),
        ToolStatusV1::Cancelled => *cancelled = cancelled.saturating_add(1),
    }
}

fn observed_total(total: u64, observed: u64, expected: u64) -> Option<u64> {
    (observed > 0 || expected == 0).then_some(total)
}

struct StructureAnalysis {
    metrics: StructureMetricsV1,
    diagnostics: Vec<TraceDiagnosticV1>,
}

fn structure_metrics(trace: &NormalizedTrace, options: &AnalyzeOptions) -> StructureAnalysis {
    let starts = trace
        .events
        .iter()
        .filter_map(|event| match &event.event {
            AgentActivityEventV1::ToolStarted(tool) => {
                Some((CallKey::new(event, &tool.call_id), tool.arguments.as_ref()))
            }
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    let finished = trace
        .events
        .iter()
        .filter_map(|event| match &event.event {
            AgentActivityEventV1::ToolFinished(tool) => Some(CallKey::new(event, &tool.call_id)),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let mut failed_edits = 0_u64;
    let mut mutation_sequences = BTreeSet::new();
    let mut mutations_by_turn = BTreeMap::<(String, u32), u64>::new();
    let mut mutation_turns_observable = true;
    let mut boundary_sequences = BTreeSet::new();
    let mut boundary_observable = true;
    let mut diagnostics = Vec::new();

    for event in &trace.events {
        let AgentActivityEventV1::ToolFinished(tool) = &event.event else {
            continue;
        };
        if tool.name == "edit" && tool.status == ToolStatusV1::Failed {
            failed_edits = failed_edits.saturating_add(1);
        }
        if matches!(tool.name.as_str(), "write" | "edit" | "apply_patch")
            && tool.status == ToolStatusV1::Succeeded
        {
            mutation_sequences.insert(event.seq);
            match event.turn {
                Some(turn) => {
                    let count = mutations_by_turn
                        .entry((event.scope.id.clone(), turn))
                        .or_default();
                    *count = count.saturating_add(1);
                }
                None => record_unavailable_structure(
                    event,
                    "successful workspace mutation completion lacks turn identity; mutation-turn metrics are unavailable",
                    &mut mutation_turns_observable,
                    &mut diagnostics,
                ),
            }
        }
        if tool.status != ToolStatusV1::Succeeded {
            continue;
        }

        if tool.name == "submit_for_pr" {
            match tool
                .result
                .as_ref()
                .and_then(|content| content_text(trace, content))
            {
                Some(text) => match accepted_submit(&text) {
                    Some(true) => {
                        boundary_sequences.insert(event.seq);
                    }
                    Some(false) => {}
                    None => record_unavailable_structure(
                        event,
                        "submit_for_pr result does not expose a recognizable accepted result",
                        &mut boundary_observable,
                        &mut diagnostics,
                    ),
                },
                None => record_unavailable_structure(
                    event,
                    "submit_for_pr result content was omitted or truncated",
                    &mut boundary_observable,
                    &mut diagnostics,
                ),
            }
        }

        if tool.name == "bash" && !options.validation_command_prefixes.is_empty() {
            let key = CallKey::new(event, &tool.call_id);
            let command = starts
                .get(&key)
                .and_then(|content| *content)
                .and_then(|content| content_text(trace, content))
                .and_then(|text| captured_command(&text));
            match command {
                Some(command)
                    if options
                        .validation_command_prefixes
                        .iter()
                        .any(|prefix| command.trim_start().starts_with(prefix)) =>
                {
                    boundary_sequences.insert(event.seq);
                }
                Some(_) => {}
                None => record_unavailable_structure(
                    event,
                    "bash arguments do not expose a complete validation command",
                    &mut boundary_observable,
                    &mut diagnostics,
                ),
            }
        }
    }

    for event in &trace.events {
        let AgentActivityEventV1::ToolStarted(tool) = &event.event else {
            continue;
        };
        if finished.contains(&CallKey::new(event, &tool.call_id)) {
            continue;
        }
        if tool.name == "submit_for_pr"
            || (tool.name == "bash" && !options.validation_command_prefixes.is_empty())
        {
            record_unavailable_structure(
                event,
                "validation-capable tool call has no observed finish event",
                &mut boundary_observable,
                &mut diagnostics,
            );
        }
    }

    let mut post_validation_mutations = 0_u64;
    let mut invalidations = 0_u64;
    let mut revalidations = 0_u64;
    let mut has_validation = false;
    let mut invalidated = false;
    for (_, kind) in ordered_structure_events(&mutation_sequences, &boundary_sequences) {
        match kind {
            StructureEvent::Boundary => {
                if invalidated {
                    revalidations = revalidations.saturating_add(1);
                }
                has_validation = true;
                invalidated = false;
            }
            StructureEvent::Mutation if has_validation => {
                post_validation_mutations = post_validation_mutations.saturating_add(1);
                if !invalidated {
                    invalidations = invalidations.saturating_add(1);
                    invalidated = true;
                }
            }
            StructureEvent::Mutation => {}
        }
    }

    let known_boundary = |value| boundary_observable.then_some(value);
    let known_mutation_turn = |value| mutation_turns_observable.then_some(value);
    let mutation_turns = mutations_by_turn.len() as u64;
    let single_mutation_turns = mutations_by_turn
        .values()
        .filter(|count| **count == 1)
        .count() as u64;
    let max_mutations_per_turn = mutations_by_turn.values().copied().max().unwrap_or(0);
    StructureAnalysis {
        metrics: StructureMetricsV1 {
            failed_edit_attempts: Some(failed_edits),
            mutations: Some(mutation_sequences.len() as u64),
            mutation_turns: known_mutation_turn(mutation_turns),
            single_mutation_turns: known_mutation_turn(single_mutation_turns),
            max_mutations_per_turn: known_mutation_turn(max_mutations_per_turn),
            validation_boundaries: known_boundary(boundary_sequences.len() as u64),
            post_validation_mutations: known_boundary(post_validation_mutations),
            validation_invalidations: known_boundary(invalidations),
            revalidations: known_boundary(revalidations),
        },
        diagnostics,
    }
}

#[derive(Clone, Copy)]
enum StructureEvent {
    Mutation,
    Boundary,
}

fn ordered_structure_events(
    mutations: &BTreeSet<u64>,
    boundaries: &BTreeSet<u64>,
) -> Vec<(u64, StructureEvent)> {
    let mut events = mutations
        .iter()
        .map(|seq| (*seq, StructureEvent::Mutation))
        .chain(
            boundaries
                .iter()
                .map(|seq| (*seq, StructureEvent::Boundary)),
        )
        .collect::<Vec<_>>();
    events.sort_by_key(|(seq, kind)| {
        (
            *seq,
            match kind {
                StructureEvent::Mutation => 0,
                StructureEvent::Boundary => 1,
            },
        )
    });
    events
}

fn content_text(trace: &NormalizedTrace, content: &CapturedContentV1) -> Option<String> {
    match content {
        CapturedContentV1::Inline(inline) if !inline.truncated && !inline.text.ends_with('…') => {
            Some(inline.text.clone())
        }
        CapturedContentV1::Inline(_) => None,
        CapturedContentV1::Blob { blob } => trace
            .attachments
            .iter()
            .find(|attachment| attachment.blob.digest == blob.digest)
            .and_then(|attachment| attachment.decode().ok())
            .and_then(|bytes| String::from_utf8(bytes).ok()),
    }
}

fn accepted_submit(text: &str) -> Option<bool> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
        if let Some(accepted) = value.get("accepted").and_then(serde_json::Value::as_bool) {
            return Some(accepted);
        }
    }
    let trimmed = text.trim_start();
    if trimmed.starts_with("submit_for_pr accepted by host:") {
        Some(true)
    } else if trimmed.starts_with("submit_for_pr rejected by host:") {
        Some(false)
    } else {
        None
    }
}

fn captured_command(text: &str) -> Option<String> {
    let command = if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
        match value {
            serde_json::Value::String(command) => nonempty(command),
            serde_json::Value::Array(argv) => argv_strings(&argv),
            serde_json::Value::Object(object) => object
                .get("command")
                .and_then(serde_json::Value::as_str)
                .and_then(|command| nonempty(command.to_string()))
                .or_else(|| {
                    object
                        .get("argv")
                        .and_then(serde_json::Value::as_array)
                        .and_then(|v| argv_strings(v))
                }),
            _ => None,
        }
    } else {
        let trimmed = text.trim();
        let unquoted = trimmed
            .strip_prefix('`')
            .and_then(|value| value.strip_suffix('`'))
            .unwrap_or(trimmed);
        nonempty(unquoted.to_string())
    };
    command.filter(|command| !command.ends_with('…'))
}

fn argv_strings(values: &[serde_json::Value]) -> Option<String> {
    values
        .iter()
        .map(serde_json::Value::as_str)
        .collect::<Option<Vec<_>>>()
        .and_then(|parts| nonempty(parts.join(" ")))
}

fn nonempty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

fn record_unavailable_structure(
    event: &AgentRunEventV1,
    message: &str,
    observable: &mut bool,
    diagnostics: &mut Vec<TraceDiagnosticV1>,
) {
    *observable = false;
    diagnostics.push(TraceDiagnosticV1 {
        code: TraceDiagnosticCodeV1::StructureEvidenceUnavailable,
        severity: DiagnosticSeverityV1::Warning,
        message: message.to_string(),
        seq: Some(event.seq),
    });
}
