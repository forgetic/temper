//! Ephemeral validator for feature #1082's locally denied shell scenario.
//!
//! Provider arguments and workspace paths remain temporary. The returned
//! evidence contains only the mapped graph checkpoints and closed lifecycle
//! facts.

use std::fs;
use std::path::Path;

use serde_json::Value as JsonValue;

use super::{FakeMcpServer, McpToolCallEvidence};

const DENIED_PROCESS_CANARY: &str = ".git/denied-shell-process-canary";

pub(super) fn validate(mcp: &FakeMcpServer, calls: &[McpToolCallEvidence]) -> Result<(), String> {
    super::mapped_graph_consumption::validate(mcp, calls)?;

    let raw = fs::read_to_string(&mcp.state_path)
        .map_err(|_| "denied-shell fixture state was unavailable".to_string())?;
    let state: JsonValue = serde_json::from_str(&raw)
        .map_err(|_| "denied-shell fixture state was malformed".to_string())?;
    let projects = state
        .get("projects")
        .and_then(JsonValue::as_object)
        .ok_or("denied-shell fixture omitted current-root state")?;
    let bindings = projects.values().collect::<Vec<_>>();
    let [binding] = bindings.as_slice() else {
        return Err("denied-shell fixture did not retain exactly one current-root binding".into());
    };
    let root = binding
        .get("repo_path")
        .and_then(JsonValue::as_str)
        .ok_or("denied-shell fixture omitted its temporary current-root path")?;
    if Path::new(root).join(DENIED_PROCESS_CANARY).exists() {
        return Err("locally denied shell invocation reached process execution".into());
    }
    Ok(())
}
