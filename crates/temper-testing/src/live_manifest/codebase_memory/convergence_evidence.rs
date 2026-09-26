//! Capture delivery and profile-specific codebase-memory evidence together.

use std::collections::BTreeMap;
use std::time::Duration;

use temper_forge_forgejo::ForgejoForge;
use temper_forge_model::{ItemNumber, RepositoryId};

use super::super::{
    FinalStateEvidence, LiveCodebaseMemoryEvidence, LivePrivacySafeCodebaseMemoryBindingEvidence,
};
use super::{
    CodebaseMemoryFake, FakeMcpServer, MEMORY_FILE, MEMORY_RESULT_NEEDLE,
    aggregate::privacy_safe_checkpoints,
    drive_codebase_memory_convergence, logged_tool_calls, privacy,
    privacy::write_privacy_safe_mcp_log,
    stable_lifecycle,
    stable_rebind::{stable_rebind_evidence, validate_mcp_contract},
};

pub(in crate::live_manifest) fn converge(
    forge: &ForgejoForge,
    repository: &RepositoryId,
    issue: ItemNumber,
    admin_user: &str,
    standalone: &mut super::super::process::ChildGuard,
    timeout: Duration,
    fake: &CodebaseMemoryFake,
    mcp: &FakeMcpServer,
) -> Result<(FinalStateEvidence, LiveCodebaseMemoryEvidence), String> {
    let final_state = drive_codebase_memory_convergence(
        forge, repository, issue, admin_user, standalone, timeout,
    )?;
    let calls = logged_tool_calls(&mcp.log_path)?;
    validate_mcp_contract(mcp, &calls)?;
    fake.validate_observations(mcp)?;
    let mut mcp_call_counts = BTreeMap::<String, usize>::new();
    for call in &calls {
        *mcp_call_counts.entry(call.name.clone()).or_default() += 1;
    }
    let basic = mcp.lifecycle_profile.as_deref() == Some("stable-lifecycle");
    let mcp_search_calls = mcp_call_counts
        .get(if basic { "search_code" } else { "search_graph" })
        .copied()
        .unwrap_or_default();
    let privacy_safe_aggregate = privacy::is_privacy_safe_profile(mcp.lifecycle_profile.as_deref());
    let aggregate_checkpoints = privacy_safe_checkpoints(mcp, &calls);
    let stable_rebind = stable_rebind_evidence(mcp, &calls)?;
    let evidence_mcp_log = if privacy_safe_aggregate {
        write_privacy_safe_mcp_log(mcp, &calls)?
    } else {
        mcp.log_path.clone()
    };
    let privacy_safe_binding = privacy_safe_aggregate
        .then(|| {
            stable_rebind
                .as_ref()
                .map(|binding| LivePrivacySafeCodebaseMemoryBindingEvidence {
                    confirmation_call_count: binding.confirmation_call_count,
                    targeted_ready_confirmation: binding.targeted_ready_confirmation,
                    current_root_rebound: binding.current_root_rebound,
                    graph_reads_use_confirmed_project: binding.graph_reads_use_confirmed_project,
                    source_reads_use_confirmed_project: binding.source_reads_use_confirmed_project,
                    source_served_from_current_root: binding.source_served_from_current_root,
                    global_inventory_avoided: binding.global_inventory_avoided,
                })
        })
        .flatten();
    let expected_result = if basic {
        stable_lifecycle::EXPECTED_RESULT.to_string()
    } else if matches!(
        mcp.lifecycle_profile.as_deref(),
        Some(
            "sequential-graph-evidence"
                | "result-driven-decision-guidance"
                | "provider-result-anchor"
                | "provider-neutral-anchor-lineage"
                | "mapped-live-graph-consumption"
                | "mapped-live-denied-shell-classification"
                | "mapped-live-ordinary-tool-convergence"
                | "mapped-live-graph-convergence"
                | "mapped-live-decision-gap-recovery"
                | "mapped-live-exact-source-selection"
                | "mapped-live-focused-test-source-relevance"
        )
    ) {
        "one successful provider-shaped graph result".to_string()
    } else {
        MEMORY_RESULT_NEEDLE.to_string()
    };
    Ok((
        final_state,
        LiveCodebaseMemoryEvidence {
            produced_file: (!privacy_safe_aggregate).then(|| {
                if basic {
                    stable_lifecycle::OUTPUT_FILE
                } else {
                    MEMORY_FILE
                }
                .to_string()
            }),
            expected_result: (!privacy_safe_aggregate).then_some(expected_result),
            fake_mcp_log: evidence_mcp_log,
            mcp_search_calls,
            mcp_call_counts: mcp_call_counts.into_iter().collect(),
            readiness_delay_ms: (!privacy_safe_aggregate).then_some(mcp.readiness_delay_ms),
            forced_failure_tool: mcp
                .forced_systemic_failure
                .as_ref()
                .map(|failure| failure.tool.clone()),
            aggregate_checkpoints,
            safe_tools: mcp
                .safe_tools
                .iter()
                .map(|tool| format!("codebase_memory_{tool}"))
                .collect(),
            hidden_tools: mcp
                .hidden_tools
                .iter()
                .map(|tool| format!("codebase_memory_{tool}"))
                .collect(),
            lifecycle: mcp.lifecycle_profile.clone(),
            stable_rebind: (!privacy_safe_aggregate).then_some(stable_rebind).flatten(),
            privacy_safe_binding,
        },
    ))
}
