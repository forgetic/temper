//! Privacy-safe retained evidence for mapped graph profiles.
//!
//! Raw MCP arguments and provider values remain in the temporary validator log.
//! Only this closed tool/checkpoint ordering is returned to scenario reporters.

use std::fs;
use std::path::PathBuf;

use super::{FakeMcpServer, McpToolCallEvidence};

pub(super) fn write_privacy_safe_mcp_log(
    mcp: &FakeMcpServer,
    calls: &[McpToolCallEvidence],
) -> Result<PathBuf, String> {
    let path = mcp
        .log_path
        .with_file_name("fake-codebase-memory-aggregate.jsonl");
    let mut safe = String::new();
    for (sequence, call) in calls.iter().enumerate() {
        safe.push_str(
            &serde_json::json!({
                "sequence": sequence + 1,
                "tool": call.name,
                "is_error": call.is_error,
                "checkpoint": call.fixture_event,
            })
            .to_string(),
        );
        safe.push('\n');
    }
    fs::write(&path, safe).map_err(|error| {
        format!(
            "write privacy-safe MCP evidence {}: {error}",
            path.display()
        )
    })?;
    Ok(path)
}

pub(super) fn privacy_safe_checkpoints(
    mcp: &FakeMcpServer,
    calls: &[McpToolCallEvidence],
) -> Vec<String> {
    if !matches!(
        mcp.lifecycle_profile.as_deref(),
        Some("mapped-live-graph-consumption" | "mapped-live-ordinary-tool-convergence")
    ) {
        return Vec::new();
    }
    let allowed = [
        "served_mapped_root",
        "served_mapped_carry_forward",
        "served_mapped_current_root_source",
        "served_mapped_unavailable",
        "served_graph_closure",
    ];
    calls
        .iter()
        .filter_map(|call| call.fixture_event.as_deref())
        .filter(|event| allowed.contains(event))
        .map(str::to_string)
        .collect()
}
