//! Ephemeral validator for #1275's route-specific recovery strengthening.
//!
//! Provider arguments, selectors, roots, values, source, and diagnostics stay
//! in temporary state. Only closed tool order and checkpoint categories cross
//! into scenario evidence.

use std::fs;
use std::path::Path;

use serde_json::Value as JsonValue;
use temper_protocol_activity::{
    GraphExplorationClosedReasonV1, GraphExplorationClosedV1, GraphRecoveryEvidenceKindV1,
    GraphRecoveryPermittedActionV1,
};

use super::stable_rebind::{confirmed_project_from_calls, validate_stable_rebind_contract};
use super::{FakeMcpServer, McpToolCallEvidence};

const DENIED_PROCESS_CANARY: &str = ".git/decision-gap-denied-shell-canary";

pub(super) fn validate(mcp: &FakeMcpServer, calls: &[McpToolCallEvidence]) -> Result<(), String> {
    let expected_tools = [
        "index_status",
        "index_repository",
        "index_status",
        "search_graph",
        "search_graph",
        "get_code_snippet",
        "trace_path",
        "get_code_snippet",
        "trace_path",
        "get_code_snippet",
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
            "decision-gap fixture requires eight successful route-coherent provider reads and no locally denied provider invocation"
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
    let first_implementation = token(&tokens, "root_a_implementation")?;
    let first_test = token(&tokens, "root_a_behavioral_test")?;
    let second_implementation = token(&tokens, "root_b_implementation")?;
    let second_caller = token(&tokens, "root_b_caller")?;
    let first_implementation_short = terminal_name(first_implementation)?;
    let second_implementation_short = terminal_name(second_implementation)?;
    let root_queries = calls[3..5]
        .iter()
        .filter_map(|call| call.arguments.get("query").and_then(JsonValue::as_str))
        .collect::<std::collections::BTreeSet<_>>();
    if root_queries
        != std::collections::BTreeSet::from([
            "focused alias retry behavior",
            "routing implementation affinity",
        ])
    {
        return Err("decision-gap fixture did not establish both transient roots".into());
    }
    let expected_arguments = [
        (5, "qualified_name", first_implementation),
        (6, "function_name", first_implementation_short),
        (7, "qualified_name", second_implementation),
        (8, "function_name", second_implementation_short),
        (9, "qualified_name", second_caller),
        (10, "qualified_name", first_test),
    ];
    if expected_arguments.iter().any(|(index, field, expected)| {
        calls[*index]
            .arguments
            .get(*field)
            .and_then(JsonValue::as_str)
            != Some(*expected)
    }) {
        return Err("decision-gap fixture did not consume its transient root selections".into());
    }
    let expected_events = [
        "served_gap_root",
        "served_gap_root",
        "served_gap_active_source",
        "served_gap_exhausted_implementation_trace",
        "served_gap_active_source",
        "served_gap_active_trace",
        "served_gap_active_source",
        "served_gap_sibling_source",
    ];
    if calls[3..]
        .iter()
        .map(|call| call.fixture_event.as_deref().unwrap_or_default())
        .collect::<Vec<_>>()
        != expected_events
    {
        return Err("decision-gap fixture omitted a closed root-coherence checkpoint".into());
    }

    validate_no_compatible_action_contract()?;
    validate_denied_shell_canary(mcp)?;
    validate_stable_rebind_contract(mcp, calls, requested)
}

fn validate_no_compatible_action_contract() -> Result<(), String> {
    let remaining = [
        GraphRecoveryEvidenceKindV1::Implementation,
        GraphRecoveryEvidenceKindV1::Caller,
        GraphRecoveryEvidenceKindV1::FocusedTest,
    ];
    let details = GraphExplorationClosedV1::exhausted(remaining)
        .ok_or("no-compatible-action details were not constructible")?;
    let no_product = temper_agent::CodingAgentError::DecisionAnchorRecoveryExhausted.to_string();
    if details.reason != GraphExplorationClosedReasonV1::RecoveryExhausted
        || details.missing_evidence != remaining
        || details.permitted_action != GraphRecoveryPermittedActionV1::StopWithoutProduct
        || details.remaining_allowance != 0
        || !details.compatible_actions.is_empty()
        || details.model_message()
            != "decision-evidence recovery exhausted; missing evidence: [implementation, caller, focused_test]; permitted action: stop_without_product; remaining allowance: 0"
        || !no_product.contains("nothing to land")
    {
        return Err(
            "no-compatible-action recovery did not terminate once with the mandatory safe no-product contract"
                .into(),
        );
    }
    Ok(())
}

fn validate_denied_shell_canary(mcp: &FakeMcpServer) -> Result<(), String> {
    let raw = fs::read_to_string(&mcp.state_path)
        .map_err(|_| "decision-gap fixture state was unavailable".to_string())?;
    let state: JsonValue = serde_json::from_str(&raw)
        .map_err(|_| "decision-gap fixture state was malformed".to_string())?;
    let projects = state
        .get("projects")
        .and_then(JsonValue::as_object)
        .ok_or("decision-gap fixture omitted current-root state")?;
    let bindings = projects.values().collect::<Vec<_>>();
    let [binding] = bindings.as_slice() else {
        return Err("decision-gap fixture did not retain exactly one current-root binding".into());
    };
    let root = binding
        .get("repo_path")
        .and_then(JsonValue::as_str)
        .ok_or("decision-gap fixture omitted its temporary current-root path")?;
    if Path::new(root).join(DENIED_PROCESS_CANARY).exists() {
        return Err("locally denied shell invocation reached process execution".into());
    }
    Ok(())
}

fn recovery_tokens(mcp: &FakeMcpServer) -> Result<serde_json::Map<String, JsonValue>, String> {
    let raw = fs::read_to_string(&mcp.state_path)
        .map_err(|_| "decision-gap fixture state was unavailable".to_string())?;
    let state: JsonValue = serde_json::from_str(&raw)
        .map_err(|_| "decision-gap fixture state was malformed".to_string())?;
    state
        .get("decision_gap_tokens")
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
