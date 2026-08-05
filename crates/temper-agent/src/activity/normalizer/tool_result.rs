use temper_protocol_activity::{CapturedContentV1, InlineContentV1};

use super::{nonempty, sanitized_text};

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
    if !matches!(name, "read" | "ls" | "grep" | "find") && !name.starts_with("codebase_memory_") {
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
