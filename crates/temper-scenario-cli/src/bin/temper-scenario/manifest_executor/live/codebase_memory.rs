// SPDX-License-Identifier: MPL-2.0

//! Privacy-aware rendering of live codebase-memory evidence.

use std::path::Path;

use temper_testing::live_manifest::LiveCodebaseMemoryEvidence;

pub(super) fn evidence_lines(
    evidence: &LiveCodebaseMemoryEvidence,
    retained_mcp_log: Option<&Path>,
) -> Vec<String> {
    let readiness_delay = evidence
        .readiness_delay_ms
        .map(|delay| format!(" readiness_delay_ms={delay}"))
        .unwrap_or_default();
    let mut lines = vec![
        format!(
            "codebase-memory tools: exposed {:?}; hidden {:?}",
            evidence.safe_tools, evidence.hidden_tools
        ),
        format!(
            "codebase-memory MCP: search_graph calls={} inventory={:?}{} forced_failure_tool={:?} log={}",
            evidence.mcp_search_calls,
            evidence.mcp_call_counts,
            readiness_delay,
            evidence.forced_failure_tool,
            retained_mcp_log.unwrap_or(&evidence.fake_mcp_log).display()
        ),
    ];
    if !evidence.aggregate_checkpoints.is_empty() {
        lines.push(format!(
            "codebase-memory aggregate checkpoints: {:?}",
            evidence.aggregate_checkpoints
        ));
    }
    lines.push(match (&evidence.produced_file, &evidence.expected_result) {
        (Some(produced_file), Some(expected_result)) => format!(
            "codebase-memory diff: engineer produced {produced_file} after bounded graph evidence containing {expected_result}"
        ),
        _ => "codebase-memory diff: minimal engineer repair followed privacy-safe result-anchor evidence"
            .to_string(),
    });
    if let Some(rebind) = &evidence.stable_rebind {
        lines.push(format!(
            "codebase-memory stable rebind: requested_project={} confirmed_project={} confirmation_calls={} targeted_discovery={} normalized_identity={} targeted_ready_confirmation={} retained_projects={} fresh_prior_binding={} current_root_rebound={} graph_reads_use_confirmed_project={} source_reads_use_confirmed_project={} source_served_from_current_root={} global_inventory_avoided={}",
            rebind.requested_stable_project,
            rebind.confirmed_provider_project,
            rebind.confirmation_call_count,
            rebind.initial_discovery_targeted,
            rebind.normalized_provider_identity,
            rebind.targeted_ready_confirmation,
            rebind.retained_project_count,
            rebind.fresh_prior_binding,
            rebind.current_root_rebound,
            rebind.graph_reads_use_confirmed_project,
            rebind.source_reads_use_confirmed_project,
            rebind.source_served_from_current_root,
            rebind.global_inventory_avoided,
        ));
    } else if let Some(binding) = &evidence.privacy_safe_binding {
        lines.push(format!(
            "codebase-memory current-root binding: confirmation_calls={} targeted_ready_confirmation={} current_root_rebound={} graph_reads_use_confirmed_project={} source_reads_use_confirmed_project={} source_served_from_current_root={} global_inventory_avoided={}",
            binding.confirmation_call_count,
            binding.targeted_ready_confirmation,
            binding.current_root_rebound,
            binding.graph_reads_use_confirmed_project,
            binding.source_reads_use_confirmed_project,
            binding.source_served_from_current_root,
            binding.global_inventory_avoided,
        ));
    }
    lines
}
