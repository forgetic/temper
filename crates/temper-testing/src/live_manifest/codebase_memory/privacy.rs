//! Privacy-safe retained evidence for mapped graph profiles.
//!
//! Raw MCP arguments and provider values remain in the temporary validator log.
//! Only this closed tool/checkpoint ordering is returned to scenario reporters.

use std::fs;
use std::path::PathBuf;

use super::{FakeMcpServer, McpToolCallEvidence};

pub(super) fn is_privacy_safe_profile(profile: Option<&str>) -> bool {
    matches!(
        profile,
        Some(
            "provider-result-anchor"
                | "provider-neutral-anchor-lineage"
                | "mapped-live-graph-consumption"
                | "mapped-live-denied-shell-classification"
                | "mapped-live-ordinary-tool-convergence"
                | "mapped-live-graph-convergence"
                | "mapped-live-decision-gap-recovery"
        )
    )
}

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
