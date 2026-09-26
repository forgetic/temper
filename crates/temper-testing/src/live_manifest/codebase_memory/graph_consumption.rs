//! Current typed source contract for the two historical consumption fixtures.
use serde_json::Value;

use super::stable_rebind::{confirmed_project_from_calls, validate_stable_rebind_contract};
use super::{FakeMcpServer, McpToolCallEvidence};

pub(super) fn validate(mcp: &FakeMcpServer, calls: &[McpToolCallEvidence]) -> Result<(), String> {
    let expected = [
        "index_status",
        "index_repository",
        "index_status",
        "search_graph",
        "search_code",
        "get_code_snippet",
        "trace_path",
        "get_code_snippet",
        "get_code_snippet",
    ];
    if calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>() != expected
        || calls
            .iter()
            .any(|c| c.is_error || c.arguments.get("decision_evidence_kind").is_some())
    {
        return Err("historical fixture requires six successful ordered graph calls and wrapper-owned typed source evidence".into());
    }
    let requested = calls[1].arguments["name"]
        .as_str()
        .filter(|s| s.starts_with("temper-v1-"))
        .ok_or("missing stable provider identity")?;
    let actual = confirmed_project_from_calls(calls, requested)?;
    validate_source_calls(mcp, &calls[3..], &actual)?;
    validate_stable_rebind_contract(mcp, calls, requested)
}

pub(super) fn validate_source_calls(
    mcp: &FakeMcpServer,
    calls: &[McpToolCallEvidence],
    actual: &str,
) -> Result<(), String> {
    if calls
        .iter()
        .map(|call| call.name.as_str())
        .collect::<Vec<_>>()
        != [
            "search_graph",
            "search_code",
            "get_code_snippet",
            "trace_path",
            "get_code_snippet",
            "get_code_snippet",
        ]
        || calls
            .iter()
            .any(|call| call.is_error || call.arguments.get("decision_evidence_kind").is_some())
    {
        return Err("historical fixture requires six successful ordered graph calls and wrapper-owned typed source evidence".into());
    }
    if calls.iter().any(|c| c.arguments["project"] != actual) {
        return Err("historical source consumption lost current-root binding".into());
    }
    let raw =
        std::fs::read_to_string(&mcp.state_path).map_err(|_| "missing historical fixture state")?;
    let state: Value =
        serde_json::from_str(&raw).map_err(|_| "malformed historical fixture state")?;
    let tokens = &state["historical_tokens"];
    let implementation = tokens["implementation"]
        .as_str()
        .ok_or("missing implementation selector")?;
    let mapped = mcp.lifecycle_profile.as_deref() == Some("mapped-live-graph-consumption");
    if calls[0].arguments["query"]
        != if mapped {
            "worker affinity routing"
        } else {
            "alias retry worker affinity"
        }
        || calls[1].arguments["pattern"] != implementation.rsplit("::").next().unwrap()
        || calls[3].arguments["direction"] != "inbound"
    {
        return Err("historical graph discovery/refinement/trace bounds changed".into());
    }
    for (index, field, token) in [
        (2, "qualified_name", "implementation"),
        (3, "function_name", "implementation"),
        (4, "qualified_name", "caller"),
        (5, "qualified_name", "focused_test"),
    ] {
        if tokens[token].as_str().is_none() || calls[index].arguments[field] != tokens[token] {
            return Err("historical fixture failed to consume a returned source selector".into());
        }
    }
    let events = if mapped {
        [
            "served_mapped_root",
            "served_mapped_carry_forward",
            "served_mapped_current_root_source",
            "served_mapped_carry_forward",
            "served_mapped_current_root_source",
            "served_mapped_current_root_source",
        ]
    } else {
        [
            "served_current_root_graph",
            "served_current_root_code_refinement",
            "served_current_root_source",
            "served_current_root_graph_trace",
            "served_current_root_source",
            "served_current_root_source",
        ]
    };
    if calls
        .iter()
        .map(|c| c.fixture_event.as_deref().unwrap_or_default())
        .collect::<Vec<_>>()
        != events
    {
        return Err("historical fixture omitted current-root source checkpoints".into());
    }
    Ok(())
}
