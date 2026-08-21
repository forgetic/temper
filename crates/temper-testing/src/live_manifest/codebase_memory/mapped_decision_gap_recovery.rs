//! Ephemeral validator for feature #1069's decision-gap recovery scenario.
//!
//! Provider arguments, values, and source stay in temporary state. Only the
//! closed call order and checkpoint categories cross into scenario evidence.

use std::fs;

use serde_json::Value as JsonValue;
use temper_protocol_activity::{
    GraphExplorationClosedReasonV1, GraphExplorationClosedV1, GraphRecoveryEvidenceKindV1,
    GraphRecoveryPermittedActionV1,
};

use super::stable_rebind::{confirmed_project_from_calls, validate_stable_rebind_contract};
use super::{FakeMcpServer, McpToolCallEvidence};

pub(super) fn validate(mcp: &FakeMcpServer, calls: &[McpToolCallEvidence]) -> Result<(), String> {
    let expected_tools = [
        "index_status",
        "index_repository",
        "index_status",
        "search_graph",
        "search_graph",
        "search_code",
        "trace_path",
        "get_code_snippet",
        "search_code",
        "search_code",
        "get_code_snippet",
    ];
    if calls
        .iter()
        .map(|call| call.name.as_str())
        .collect::<Vec<_>>()
        != expected_tools
        || calls.iter().any(|call| call.is_error)
    {
        return Err(
            "decision-gap fixture requires eight successful provider reads and no locally denied provider invocation"
                .into(),
        );
    }
    if calls
        .iter()
        .any(|call| call.arguments.get("decision_evidence_kind").is_some())
    {
        return Err("wrapper-owned decision evidence reached the MCP provider".into());
    }

    let requested = calls[1]
        .arguments
        .get("name")
        .and_then(JsonValue::as_str)
        .filter(|name| name.starts_with("temper-v1-"))
        .ok_or("decision-gap fixture did not use a stable provider identity")?;
    let confirmed = confirmed_project_from_calls(calls, requested)?;
    if calls[3..].iter().any(|call| {
        call.arguments.get("project").and_then(JsonValue::as_str) != Some(confirmed.as_str())
    }) {
        return Err("decision-gap fixture lost its confirmed current-root binding".into());
    }

    let tokens = recovery_tokens(mcp)?;
    let implementation = token(&tokens, "implementation")?;
    let caller = token(&tokens, "caller")?;
    let focused_test = token(&tokens, "behavioral_test")?;
    let implementation_short = terminal_name(implementation)?;
    let expected_arguments = [
        (3, "query", "routing implementation affinity"),
        (4, "query", "focused alias retry behavior"),
        (5, "pattern", implementation_short),
        (6, "function_name", implementation_short),
        (7, "qualified_name", focused_test),
        (8, "pattern", implementation_short),
        (9, "pattern", implementation_short),
        (10, "qualified_name", caller),
    ];
    if expected_arguments.iter().any(|(index, field, expected)| {
        calls[*index]
            .arguments
            .get(*field)
            .and_then(JsonValue::as_str)
            != Some(*expected)
    }) {
        return Err(
            "decision-gap fixture did not consume its transient provider selections".into(),
        );
    }

    let expected_events = [
        "served_gap_root",
        "served_gap_root",
        "served_gap_refinement",
        "served_gap_trace",
        "served_gap_source",
        "served_gap_duplicate",
        "served_gap_duplicate",
        "served_gap_recovery_source",
    ];
    if calls[3..]
        .iter()
        .map(|call| call.fixture_event.as_deref().unwrap_or_default())
        .collect::<Vec<_>>()
        != expected_events
    {
        return Err("decision-gap fixture omitted a closed aggregate checkpoint".into());
    }

    validate_safe_stop_contract()?;
    validate_stable_rebind_contract(mcp, calls, requested)
}

fn validate_safe_stop_contract() -> Result<(), String> {
    let details = GraphExplorationClosedV1::exhausted([GraphRecoveryEvidenceKindV1::Caller])
        .ok_or("safe-stop details were not constructible")?;
    let no_product = temper_agent::CodingAgentError::DecisionAnchorRecoveryExhausted.to_string();
    if details.reason != GraphExplorationClosedReasonV1::RecoveryExhausted
        || details.missing_evidence != [GraphRecoveryEvidenceKindV1::Caller]
        || details.permitted_action != GraphRecoveryPermittedActionV1::StopWithoutProduct
        || details.remaining_allowance != 0
        || details.model_message()
            != "decision-evidence recovery exhausted; missing evidence: [caller]; permitted action: stop_without_product; remaining allowance: 0"
        || !no_product.contains("nothing to land")
    {
        return Err(
            "recovery exhaustion did not retain the mandatory safe no-product contract".into(),
        );
    }
    Ok(())
}

fn recovery_tokens(mcp: &FakeMcpServer) -> Result<serde_json::Map<String, JsonValue>, String> {
    let raw = fs::read_to_string(&mcp.state_path)
        .map_err(|_| "decision-gap fixture state was unavailable".to_string())?;
    let state: JsonValue = serde_json::from_str(&raw)
        .map_err(|_| "decision-gap fixture state was malformed".to_string())?;
    state
        .get("graph_convergence_tokens")
        .and_then(JsonValue::as_object)
        .cloned()
        .ok_or("decision-gap fixture omitted transient selections".to_string())
}

fn token<'a>(
    tokens: &'a serde_json::Map<String, JsonValue>,
    name: &str,
) -> Result<&'a str, String> {
    tokens
        .get(name)
        .and_then(JsonValue::as_str)
        .ok_or("decision-gap fixture omitted a transient selection".to_string())
}

fn terminal_name(qualified: &str) -> Result<&str, String> {
    qualified
        .rsplit_once("::")
        .map(|(_, terminal)| terminal)
        .filter(|terminal| !terminal.is_empty())
        .ok_or("decision-gap fixture selection was not transformable".to_string())
}
