// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeMap;

use crate::RunSummaryV1;

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
    "failed_edit_attempts",
    "mutations",
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
];

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
    if let Some(structure) = &summary.metrics.structure {
        if let Some(value) = structure.failed_edit_attempts {
            values.insert("failed_edit_attempts", value);
        }
        if let Some(value) = structure.mutations {
            values.insert("mutations", value);
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
