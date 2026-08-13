use temper_protocol_activity::{DecisionAnchorLineageV1, GraphCorrelationV1};
use tongs::tools::ToolOutput;

use crate::machine::{
    CODEBASE_MEMORY_TOOL_PREFIX, CodebaseMemoryTiming, SAFE_DECISION_ANCHOR_LINEAGE_DETAIL_KEY,
    SAFE_GRAPH_CORRELATION_DETAIL_KEY, SAFE_TOOL_FAILURE_DETAIL_KEY, ToolFailureCategory,
    ToolFailureDiagnostic, ToolResultMetadata,
};

pub(super) const TOOL_RESULT_PREVIEW_BYTES: usize = 4 * 1024;
pub(super) const OPERATOR_GRAPH_RESULT_CAPTURE_BYTES: usize = 16 * 1024;

pub(super) fn bounded_result_text(
    output: &ToolOutput,
    maximum_bytes: usize,
) -> Option<(String, bool)> {
    let text = result_text(output);
    (!text.is_empty()).then(|| {
        let (bounded, truncated) = truncate_utf8(&text, maximum_bytes);
        (bounded.to_string(), truncated)
    })
}

/// Extract a bounded text-only candidate from a tool result. Generic structured
/// details, signatures, images, and arbitrary JSON never enter the event
/// protocol. A codebase-memory wrapper may contribute only a stable category,
/// bounded numeric timing fields, and a closed argument fingerprint.
pub(super) fn bounded_tool_result(name: &str, output: &ToolOutput) -> ToolResultMetadata {
    let failure = safe_tool_failure(name, output);
    let codebase_memory_timing = codebase_memory_timing(name, output);
    let graph_correlation = graph_correlation(name, output);
    let decision_anchor_lineage = decision_anchor_lineage(name, output, graph_correlation.as_ref());
    let text = result_text(output);
    let bytes = u64::try_from(text.len()).unwrap_or(u64::MAX);
    if text.is_empty() {
        return ToolResultMetadata {
            preview: None,
            bytes,
            truncated: false,
            failure,
            codebase_memory_timing,
            graph_correlation,
            decision_anchor_lineage,
        };
    }
    // Graph result text has one explicit private consumer in the executor.
    // Keeping it out of event metadata also keeps activity, lineage, and Debug
    // paths content-free without relying on every downstream projection.
    let graph_result = name.starts_with(CODEBASE_MEMORY_TOOL_PREFIX);
    let (preview, truncated) = if graph_result {
        (None, text.len() > OPERATOR_GRAPH_RESULT_CAPTURE_BYTES)
    } else {
        let (preview, truncated) = truncate_utf8(&text, TOOL_RESULT_PREVIEW_BYTES);
        (Some(preview.to_string()), truncated)
    };
    ToolResultMetadata {
        preview,
        bytes,
        truncated,
        failure,
        codebase_memory_timing,
        graph_correlation,
        decision_anchor_lineage,
    }
}

fn result_text(output: &ToolOutput) -> String {
    output
        .content
        .iter()
        .filter_map(|block| match block {
            tongs::model::ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn decision_anchor_lineage(
    name: &str,
    output: &ToolOutput,
    correlation: Option<&GraphCorrelationV1>,
) -> Option<DecisionAnchorLineageV1> {
    if !name.starts_with(CODEBASE_MEMORY_TOOL_PREFIX) || output.is_error {
        return None;
    }
    let lineage: DecisionAnchorLineageV1 = serde_json::from_value(
        output
            .details
            .as_ref()?
            .get(SAFE_DECISION_ANCHOR_LINEAGE_DETAIL_KEY)?
            .clone(),
    )
    .ok()?;
    lineage.is_valid_for(correlation?).then_some(lineage)
}

fn graph_correlation(name: &str, output: &ToolOutput) -> Option<GraphCorrelationV1> {
    if !name.starts_with(CODEBASE_MEMORY_TOOL_PREFIX) || output.is_error {
        return None;
    }
    let correlation: GraphCorrelationV1 = serde_json::from_value(
        output
            .details
            .as_ref()?
            .get(SAFE_GRAPH_CORRELATION_DETAIL_KEY)?
            .clone(),
    )
    .ok()?;
    (correlation.is_valid() && correlation.tool.public_name() == name).then_some(correlation)
}

fn codebase_memory_timing(name: &str, output: &ToolOutput) -> Option<CodebaseMemoryTiming> {
    if !name.starts_with(CODEBASE_MEMORY_TOOL_PREFIX) {
        return None;
    }
    let timing = output.details.as_ref()?.get("timing")?;
    Some(CodebaseMemoryTiming {
        readiness_wait_ms: timing.get("readiness_wait_ms")?.as_u64()?,
        graph_execution_ms: timing.get("graph_execution_ms")?.as_u64()?,
    })
}

fn safe_tool_failure(name: &str, output: &ToolOutput) -> Option<ToolFailureDiagnostic> {
    if !output.is_error || !name.starts_with(CODEBASE_MEMORY_TOOL_PREFIX) {
        return None;
    }
    let marker = output.details.as_ref()?.get(SAFE_TOOL_FAILURE_DETAIL_KEY)?;
    if marker.get("source").and_then(serde_json::Value::as_str) != Some("codebase_memory") {
        return None;
    }
    let category = marker
        .get("category")
        .and_then(serde_json::Value::as_str)
        .and_then(ToolFailureCategory::from_stable_str)?;
    Some(ToolFailureDiagnostic::codebase_memory(category))
}

fn truncate_utf8(value: &str, maximum_bytes: usize) -> (&str, bool) {
    if value.len() <= maximum_bytes {
        return (value, false);
    }
    let mut end = maximum_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    (&value[..end], true)
}
