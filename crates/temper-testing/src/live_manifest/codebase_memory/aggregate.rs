//! Privacy-safe aggregate projection for mapped codebase-memory fixtures.

use super::{FakeMcpServer, McpToolCallEvidence};

pub(super) fn privacy_safe_checkpoints(
    mcp: &FakeMcpServer,
    calls: &[McpToolCallEvidence],
) -> Vec<String> {
    let allowed: &[&str] = match mcp.lifecycle_profile.as_deref() {
        Some("mapped-live-graph-consumption" | "mapped-live-ordinary-tool-convergence") => &[
            "served_mapped_root",
            "served_mapped_carry_forward",
            "served_mapped_current_root_source",
        ],
        Some("mapped-live-graph-convergence") => &[
            "served_convergence_preflight_root",
            "served_convergence_preflight_trace",
            "served_convergence_unavailable",
            "served_convergence_root",
            "served_convergence_refinement",
            "served_convergence_trace",
            "served_convergence_duplicate",
            "served_convergence_source",
        ],
        _ => return Vec::new(),
    };
    calls
        .iter()
        .filter_map(|call| call.fixture_event.as_deref())
        .filter(|event| allowed.contains(event))
        .map(str::to_string)
        .collect()
}
