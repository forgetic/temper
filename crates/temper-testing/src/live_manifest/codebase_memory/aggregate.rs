//! Privacy-safe aggregate projection for mapped codebase-memory fixtures.

use super::{FakeMcpServer, McpToolCallEvidence};

pub(super) fn is_current_root_source_checkpoint(checkpoint: Option<&str>) -> bool {
    matches!(
        checkpoint,
        Some(
            "served_current_root_source"
                | "served_result_derived_consumer"
                | "served_typed_lineage_consumer"
                | "served_mapped_current_root_source"
                | "served_convergence_source"
                | "served_gap_sibling_source"
                | "served_gap_active_source"
                | "served_focus_implementation_source"
                | "served_focus_caller_source"
                | "served_focus_test_source"
                | "served_selection_implementation_source"
                | "served_selection_caller_source"
                | "served_selection_focused_source"
                | "served_selection_active_source"
                | "served_selection_provisional_preview"
                | "served_selection_provisional_source"
                | "served_selection_correction_preview"
                | "served_selection_corrected_source"
        )
    )
}

pub(super) fn privacy_safe_checkpoints(
    mcp: &FakeMcpServer,
    calls: &[McpToolCallEvidence],
) -> Vec<String> {
    let allowed: &[&str] = match mcp.lifecycle_profile.as_deref() {
        Some(
            "mapped-live-graph-consumption"
            | "mapped-live-denied-shell-classification"
            | "mapped-live-ordinary-tool-convergence",
        ) => &[
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
        Some("mapped-live-decision-gap-recovery") => &[
            "served_gap_root",
            "served_gap_sibling_source",
            "served_gap_exhausted_implementation_trace",
            "served_gap_active_trace",
            "served_gap_active_source",
        ],
        Some("mapped-live-exact-source-selection") => &[
            "served_selection_root",
            "served_selection_provisional_preview",
            "served_selection_provisional_source",
            "served_selection_correction_trace",
            "served_selection_caller_source",
            "served_selection_focused_source",
            "served_selection_correction_preview",
            "served_selection_corrected_source",
        ],
        Some("mapped-live-focused-test-source-relevance") => &[
            "served_focus_root",
            "served_focus_implementation_source",
            "served_focus_caller_trace",
            "served_focus_caller_source",
            "served_focus_non_progress",
            "served_focus_empty_traversal",
            "served_focus_fallback",
            "served_focus_test_source",
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
