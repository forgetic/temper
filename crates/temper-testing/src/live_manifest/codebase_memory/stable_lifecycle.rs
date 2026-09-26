//! The original single-search fixture has its own readiness and delivery contract.

use super::{FakeMcpServer, McpToolCallEvidence, ModelObservations};

pub(super) const OUTPUT_FILE: &str = "MEMORY_NOTES.md";
pub(super) const EXPECTED_RESULT: &str = "one verified current-root search result";
pub(super) const ENGINEER_SUMMARY: &str =
    "Used codebase memory search result before writing MEMORY_NOTES.md.";
pub(super) const PATTERN: &str = "WidgetService";

pub(super) fn validate(mcp: &FakeMcpServer, calls: &[McpToolCallEvidence]) -> Result<(), String> {
    let [discovery, index, confirmation, search] = calls else {
        return Err(
            "stable lifecycle requires exactly discovery, index, confirmation and search".into(),
        );
    };
    let names = calls
        .iter()
        .map(|call| call.name.as_str())
        .collect::<Vec<_>>();
    if names
        != [
            "index_status",
            "index_repository",
            "index_status",
            "search_code",
        ]
    {
        return Err("stable lifecycle changed its four-call readiness/search order".into());
    }
    let project = index.arguments["name"]
        .as_str()
        .filter(|name| name.starts_with("temper-v1-"))
        .ok_or("stable lifecycle index omitted its stable project")?;
    let root = index.arguments["repo_path"]
        .as_str()
        .filter(|root| !root.is_empty())
        .ok_or("stable lifecycle index omitted its checkout root")?;
    if project != mcp.project
        || [discovery, confirmation, search]
            .iter()
            .any(|call| call.arguments["project"] != project)
        || !discovery.is_error
        || index.is_error
        || confirmation.is_error
        || search.is_error
        || index.delay_ms != Some(mcp.readiness_delay_ms)
        || confirmation.fixture_event.as_deref() != Some("current_root_confirmed")
        || search.fixture_event.as_deref() != Some("served_stable_current_root_search")
        || search.arguments["pattern"] != PATTERN
        || search.arguments.get("query").is_some()
    {
        return Err(
            "stable lifecycle lost its missing-to-ready current-root search contract".into(),
        );
    }
    let state: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&mcp.state_path)
            .map_err(|error| format!("read stable lifecycle state: {error}"))?,
    )
    .map_err(|error| format!("parse stable lifecycle state: {error}"))?;
    let projects = state["projects"]
        .as_object()
        .ok_or("stable lifecycle state omitted projects")?;
    if projects.len() != 1
        || state["projects"][project]["repo_path"] != root
        || state["projects"][project]["binding"] != "current_prepared_checkout"
        || state["projects"][project]["requested_stable_project"] != project
        || state["counters"]["project_creations"] != 1
        || state["counters"]["rebinds"] != 1
    {
        return Err(
            "stable lifecycle did not retain exactly its indexed current-root binding".into(),
        );
    }
    Ok(())
}

pub(super) fn validate_observations(
    observations: &ModelObservations,
    engineer_requests: usize,
) -> Result<(), String> {
    if !observations.prompt_guidance_seen
        || !observations.memory_result_seen
        || !observations.current_root_source_seen
        || observations.current_root_source_results != 1
        || observations.raw_provider_text_seen
        || observations.oversized_message_seen
        || engineer_requests != 4
    {
        return Err(
            "stable lifecycle did not consume one verified source before its four-reply delivery"
                .into(),
        );
    }
    Ok(())
}

#[cfg(test)]
#[path = "stable_lifecycle_tests.rs"]
mod tests;
