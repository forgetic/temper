use std::path::{Path, PathBuf};

use temper_protocol_agent::AgentRuntimeLimitsV1;

use crate::agent_runner::AgentRunError;

pub(super) fn write(
    temp_root: &Path,
    limits: Option<AgentRuntimeLimitsV1>,
) -> Result<Option<PathBuf>, AgentRunError> {
    let Some(limits) = limits else {
        return Ok(None);
    };
    let path = temp_root.join("runtime-limits.json");
    let bytes = serde_json::to_vec_pretty(&limits).map_err(|error| {
        AgentRunError::transient(format!("serialize agent runtime limits: {error}"))
    })?;
    std::fs::write(&path, bytes).map_err(|error| {
        AgentRunError::transient(format!("write agent runtime limits file: {error}"))
    })?;
    Ok(Some(path))
}
