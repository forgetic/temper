// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeMap;

use temper_protocol_activity::ToolFailureCategoryV1;

use crate::{RunSummaryV1, RunTerminalStatusV1};

/// Stable structural metrics shown in the primary aggregate/comparison table.
pub const PRIMARY_METRICS: &[&str] = &[
    "turns",
    "model_attempts",
    "model_failed_attempts",
    "model_retries",
    "provider_failures",
    "input_tokens",
    "output_tokens",
    "cache_read_tokens",
    "cache_write_tokens",
    "tool_calls",
    "failed_tool_calls",
    "task_correct",
    "host_validation_commands",
    "host_validation_failures",
    "graph_calls",
    "graph_succeeded_calls",
    "graph_failed_calls",
    "graph_cancelled_calls",
    "graph_relevant_results",
    "graph_irrelevant_successes",
    "graph_immediate_repeats_after_systemic_failure",
    "conventional_discovery_calls_before_selection",
    "conventional_grep_calls_before_selection",
    "conventional_find_calls_before_selection",
    "conventional_read_calls_before_selection",
    "conventional_shell_segments_before_selection",
    "graph_failure_configuration_startup",
    "graph_failure_project_not_ready",
    "graph_failure_index_failure",
    "graph_failure_timeout",
    "graph_failure_transport",
    "graph_failure_process_exit",
    "graph_failure_provider_protocol",
    "graph_failure_invalid_model_input",
    "graph_failure_circuit_open",
    "failed_edit_attempts",
    "mutations",
    "mutation_turns",
    "single_mutation_turns",
    "max_mutations_per_turn",
    "validation_invalidations",
    "diff_files_changed",
    "diff_insertions",
    "diff_deletions",
];

/// Timing metrics are intentionally advisory because provider and host noise can
/// dominate small samples.
pub const ADVISORY_METRICS: &[&str] = &[
    "wall_time_ms",
    "model_duration_ms",
    "model_time_to_first_token_ms",
    "tool_duration_ms",
    "graph_readiness_wait_ms",
    "graph_discovery_duration_ms",
];

fn task_correctness(summary: &RunSummaryV1) -> Option<bool> {
    match summary.terminal.as_ref().map(|terminal| terminal.status) {
        Some(RunTerminalStatusV1::Failed | RunTerminalStatusV1::Cancelled) => Some(false),
        Some(RunTerminalStatusV1::Succeeded) => {
            summary.validation.as_ref().and_then(|validation| {
                (validation.command_count > 0).then_some(validation.failed == 0)
            })
        }
        None => None,
    }
}

fn all_failure_categories() -> [ToolFailureCategoryV1; 9] {
    [
        ToolFailureCategoryV1::ConfigurationStartup,
        ToolFailureCategoryV1::ProjectNotReady,
        ToolFailureCategoryV1::IndexFailure,
        ToolFailureCategoryV1::Timeout,
        ToolFailureCategoryV1::Transport,
        ToolFailureCategoryV1::ProcessExit,
        ToolFailureCategoryV1::ProviderProtocol,
        ToolFailureCategoryV1::InvalidModelInput,
        ToolFailureCategoryV1::CircuitOpen,
    ]
}

fn failure_metric(category: ToolFailureCategoryV1) -> &'static str {
    match category {
        ToolFailureCategoryV1::ConfigurationStartup => "graph_failure_configuration_startup",
        ToolFailureCategoryV1::ProjectNotReady => "graph_failure_project_not_ready",
        ToolFailureCategoryV1::IndexFailure => "graph_failure_index_failure",
        ToolFailureCategoryV1::Timeout => "graph_failure_timeout",
        ToolFailureCategoryV1::Transport => "graph_failure_transport",
        ToolFailureCategoryV1::ProcessExit => "graph_failure_process_exit",
        ToolFailureCategoryV1::ProviderProtocol => "graph_failure_provider_protocol",
        ToolFailureCategoryV1::InvalidModelInput => "graph_failure_invalid_model_input",
        ToolFailureCategoryV1::CircuitOpen => "graph_failure_circuit_open",
    }
}

fn has_full_coverage(coverage: &crate::MetricCoverageV1) -> bool {
    coverage.expected == Some(coverage.observed)
}

pub(crate) fn metric_values(summary: &RunSummaryV1) -> BTreeMap<&'static str, u64> {
    let mut values = BTreeMap::new();
    if let Some(value) = summary.metrics.turns {
        values.insert("turns", value);
    }
    if let Some(model) = &summary.metrics.model {
        values.insert("model_attempts", model.attempts);
        values.insert("model_failed_attempts", model.failed_attempts);
        values.insert("model_retries", model.retries);
        values.insert("provider_failures", model.provider_failures);
        if let Some(value) = model.cumulative_duration_ms {
            values.insert("model_duration_ms", value);
        }
        if let Some(value) = model.cumulative_time_to_first_token_ms {
            values.insert("model_time_to_first_token_ms", value);
        }
    }
    if let Some(tokens) = &summary.metrics.tokens {
        values.insert("input_tokens", tokens.input_tokens);
        values.insert("output_tokens", tokens.output_tokens);
        values.insert("cache_read_tokens", tokens.cache_read_tokens);
        values.insert("cache_write_tokens", tokens.cache_write_tokens);
    }
    if let Some(tools) = &summary.metrics.tools {
        values.insert("tool_calls", tools.calls);
        values.insert("failed_tool_calls", tools.failed);
        if let Some(value) = tools.cumulative_duration_ms {
            values.insert("tool_duration_ms", value);
        }
    }
    if let Some(correct) = task_correctness(summary) {
        values.insert("task_correct", u64::from(correct));
    }
    if let Some(validation) = &summary.validation {
        values.insert("host_validation_commands", validation.command_count);
        values.insert("host_validation_failures", validation.failed);
    }
    if let Some(graph) = &summary.metrics.graph {
        values.insert("graph_calls", graph.calls);
        if has_full_coverage(&graph.status_coverage) {
            values.insert("graph_succeeded_calls", graph.succeeded);
            values.insert("graph_failed_calls", graph.failed);
            values.insert("graph_cancelled_calls", graph.cancelled);
        }
        if has_full_coverage(&graph.relevance_coverage) {
            if let Some(value) = graph.relevant_results {
                values.insert("graph_relevant_results", value);
            }
            if let Some(value) = graph.irrelevant_successes {
                values.insert("graph_irrelevant_successes", value);
            }
        }
        if has_full_coverage(&graph.status_coverage)
            && has_full_coverage(&graph.immediate_repeat_coverage)
        {
            if let Some(value) = graph.immediate_repeated_attempts_after_systemic_failure {
                values.insert("graph_immediate_repeats_after_systemic_failure", value);
            }
        }
        if has_full_coverage(&graph.status_coverage)
            && has_full_coverage(&graph.failure_category_coverage)
        {
            for category in all_failure_categories() {
                values.insert(
                    failure_metric(category),
                    graph
                        .failures_by_category
                        .get(&category)
                        .copied()
                        .unwrap_or(0),
                );
            }
        }
        if has_full_coverage(&graph.readiness_wait_coverage) {
            if let Some(value) = graph.cumulative_readiness_wait_ms {
                values.insert("graph_readiness_wait_ms", value);
            }
        }
        if has_full_coverage(&graph.discovery_duration_coverage) {
            if let Some(value) = graph.cumulative_discovery_duration_ms {
                values.insert("graph_discovery_duration_ms", value);
            }
        }
        if let Some(discovery) = &graph.conventional_discovery_before_selection {
            if let Some(value) = discovery.total_calls {
                values.insert("conventional_discovery_calls_before_selection", value);
            }
            values.insert(
                "conventional_grep_calls_before_selection",
                discovery.grep_calls,
            );
            values.insert(
                "conventional_find_calls_before_selection",
                discovery.find_calls,
            );
            values.insert(
                "conventional_read_calls_before_selection",
                discovery.read_calls,
            );
            if has_full_coverage(&discovery.shell_command_classification_coverage) {
                values.insert(
                    "conventional_shell_segments_before_selection",
                    discovery.classified_shell_segments,
                );
            }
        }
    }
    if let Some(structure) = &summary.metrics.structure {
        if let Some(value) = structure.failed_edit_attempts {
            values.insert("failed_edit_attempts", value);
        }
        if let Some(value) = structure.mutations {
            values.insert("mutations", value);
        }
        if let Some(value) = structure.mutation_turns {
            values.insert("mutation_turns", value);
        }
        if let Some(value) = structure.single_mutation_turns {
            values.insert("single_mutation_turns", value);
        }
        if let Some(value) = structure.max_mutations_per_turn {
            values.insert("max_mutations_per_turn", value);
        }
        if let Some(value) = structure.validation_invalidations {
            values.insert("validation_invalidations", value);
        }
    }
    if let Some(diff) = &summary.diff {
        values.insert("diff_files_changed", diff.files_changed);
        values.insert("diff_insertions", diff.insertions);
        values.insert("diff_deletions", diff.deletions);
    }
    if let Some(value) = summary.wall_time_ms {
        values.insert("wall_time_ms", value);
    }
    values
}
