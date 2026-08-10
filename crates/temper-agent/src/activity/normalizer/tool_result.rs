use temper_agent_core::{CodebaseMemoryTiming, ToolCallStatus};
use temper_protocol_activity::{
    CapturedContentV1, CodebaseMemoryTimingV1, GraphCorrelationV1, InlineContentV1,
};

use super::{nonempty, sanitized_text};

pub(super) fn graph_timing(timing: Option<CodebaseMemoryTiming>) -> Option<CodebaseMemoryTimingV1> {
    timing.map(|timing| CodebaseMemoryTimingV1 {
        readiness_wait_ms: timing.readiness_wait_ms,
        graph_execution_ms: timing.graph_execution_ms,
    })
}

pub(super) fn graph_correlation(
    correlation: Option<GraphCorrelationV1>,
    name: &str,
    status: ToolCallStatus,
) -> Option<GraphCorrelationV1> {
    correlation.filter(|correlation| {
        status == ToolCallStatus::Succeeded
            && correlation.is_valid()
            && correlation.tool.public_name() == name
    })
}

pub(super) fn captured_tool_result(
    name: &str,
    preview: Option<String>,
    truncated: bool,
    maximum_bytes: usize,
) -> Option<CapturedContentV1> {
    let value = nonempty(preview?)?;
    if name == "submit_for_pr" {
        return submit_result_marker(&value).map(|marker| {
            CapturedContentV1::Inline(InlineContentV1 {
                text: marker.to_string(),
                truncated: false,
            })
        });
    }
    // Provider-shaped graph results and their model-visible anchors may contain
    // source, paths, and selected identities. Durable activity keeps only the
    // closed correlation, typed failure, and timing facts below.
    if name.starts_with("codebase_memory_") {
        return None;
    }
    if !matches!(name, "read" | "ls" | "grep" | "find") {
        return None;
    }
    let mut inline = sanitized_text(&value, maximum_bytes);
    inline.truncated |= truncated;
    Some(CapturedContentV1::Inline(inline))
}

fn submit_result_marker(value: &str) -> Option<&'static str> {
    let trimmed = value.trim_start();
    if trimmed.starts_with("submit_for_pr accepted by host:") {
        Some("submit_for_pr accepted by host:")
    } else if trimmed.starts_with("submit_for_pr rejected by host:") {
        Some("submit_for_pr rejected by host:")
    } else {
        None
    }
}
