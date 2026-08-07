use std::fs;

use serde_json::Value as JsonValue;

use super::super::LiveStableRebindEvidence;
use super::{FakeMcpServer, McpToolCallEvidence};

pub(super) fn stable_rebind_evidence(
    mcp: &FakeMcpServer,
    calls: &[McpToolCallEvidence],
) -> Result<Option<LiveStableRebindEvidence>, String> {
    if mcp.lifecycle_profile.as_deref() != Some("stable-rebind") {
        return Ok(None);
    }
    let stable_project = calls
        .iter()
        .find(|call| call.name == "index_repository")
        .and_then(|call| call.arguments.get("name"))
        .and_then(JsonValue::as_str)
        .ok_or("stable-rebind index_repository call omitted stable project name")?
        .to_string();
    let state = read_stable_rebind_state(mcp)?;
    let retained_project_count = state
        .get("projects")
        .and_then(JsonValue::as_object)
        .map_or(0, |projects| projects.len());
    Ok(Some(LiveStableRebindEvidence {
        stable_project,
        retained_project_count,
        fresh_prior_binding: calls.iter().any(|call| {
            call.name == "index_status"
                && call.fixture_event.as_deref() == Some("fresh_prior_binding")
        }),
        current_root_rebound: calls.iter().any(|call| {
            call.name == "index_repository"
                && call.fixture_event.as_deref() == Some("current_root_rebound")
        }),
        source_served_from_current_root: calls.iter().any(|call| {
            call.name == "get_code_snippet"
                && call.fixture_event.as_deref() == Some("served_current_root_source")
        }),
        global_inventory_avoided: calls.iter().all(|call| call.name != "list_projects"),
    }))
}

pub(super) fn validate_stable_rebind_contract(
    mcp: &FakeMcpServer,
    calls: &[McpToolCallEvidence],
    provider_project: &str,
) -> Result<(), String> {
    let evidence = stable_rebind_evidence(mcp, calls)?
        .ok_or("stable-rebind fixture did not produce rebind evidence")?;
    if evidence.stable_project != provider_project
        || !evidence.fresh_prior_binding
        || !evidence.current_root_rebound
        || !evidence.source_served_from_current_root
        || !evidence.global_inventory_avoided
        || evidence.retained_project_count != 1
    {
        return Err(format!(
            "stable-rebind fixture did not retain one fresh stable project, rebind it to the current checkout, and serve source without inventory discovery: {evidence:?}"
        ));
    }
    let index_root = calls
        .iter()
        .find(|call| call.name == "index_repository")
        .and_then(|call| call.arguments.get("repo_path"))
        .and_then(JsonValue::as_str)
        .ok_or("stable-rebind index_repository call omitted repo_path")?;
    let state = read_stable_rebind_state(mcp)?;
    let binding = state
        .get("projects")
        .and_then(JsonValue::as_object)
        .and_then(|projects| projects.get(provider_project))
        .ok_or("stable-rebind fixture omitted retained provider project")?;
    if binding.get("repo_path").and_then(JsonValue::as_str) != Some(index_root)
        || binding.get("binding").and_then(JsonValue::as_str) != Some("current_prepared_checkout")
        || state
            .get("counters")
            .and_then(|counters| counters.get("project_creations"))
            .and_then(JsonValue::as_u64)
            != Some(1)
        || state
            .get("counters")
            .and_then(|counters| counters.get("rebinds"))
            .and_then(JsonValue::as_u64)
            != Some(1)
    {
        return Err(
            "stable-rebind fixture did not replace the prior checkout binding with the active root under one stable project"
                .to_string(),
        );
    }
    Ok(())
}

fn read_stable_rebind_state(mcp: &FakeMcpServer) -> Result<JsonValue, String> {
    let raw = fs::read_to_string(&mcp.state_path).map_err(|error| {
        format!(
            "read stable-rebind fixture state {}: {error}",
            mcp.state_path.display()
        )
    })?;
    serde_json::from_str(&raw).map_err(|error| {
        format!(
            "parse stable-rebind fixture state {}: {error}",
            mcp.state_path.display()
        )
    })
}
