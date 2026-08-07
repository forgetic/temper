use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};
use temper_agent_core::AgentContainmentContext;

use crate::mcp::{McpError, McpToolCallResult, StdioMcpClient, StdioMcpServerConfig};

use super::indexing::stable_index_arguments;

const MAX_INDEX_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_PROVIDER_IDENTITY_BYTES: usize = 512;

/// Confirms that an upsert's provider-selected identity is ready and bound to
/// the current canonical checkout before graph reads can use it.
pub(super) async fn confirm_current_root_binding(
    mcp_config: &StdioMcpServerConfig,
    expected_root: &Path,
    provider_key: &str,
    timeout: Duration,
    containment: &AgentContainmentContext,
) -> Result<String, McpError> {
    let started = Instant::now();
    let client =
        StdioMcpClient::connect_with_containment(mcp_config.clone(), containment.clone()).await?;
    let upsert = client
        .call_tool(
            "index_repository",
            stable_index_arguments(expected_root, provider_key),
            remaining_budget(timeout, started)?,
        )
        .await?;
    let actual_project = confirmed_upsert_identity(&upsert, provider_key)?;
    let status = client
        .call_tool(
            "index_status",
            json!({ "project": actual_project }),
            remaining_budget(timeout, started)?,
        )
        .await?;
    confirm_index_status(&status, provider_key, &actual_project, expected_root)?;
    Ok(actual_project)
}

/// Blocking counterpart used by the background worker. Both provider calls
/// consume the same indexing budget, just as the async path does.
pub(super) fn confirm_current_root_binding_blocking(
    mcp_config: StdioMcpServerConfig,
    expected_root: &Path,
    provider_key: &str,
    timeout: Duration,
    containment: AgentContainmentContext,
) -> Result<String, McpError> {
    let started = Instant::now();
    let client = StdioMcpClient::connect_blocking_with_containment(mcp_config, containment)?;
    let upsert = client.call_tool_blocking(
        "index_repository",
        stable_index_arguments(expected_root, provider_key),
        remaining_budget(timeout, started)?,
    )?;
    let actual_project = confirmed_upsert_identity(&upsert, provider_key)?;
    let status = client.call_tool_blocking(
        "index_status",
        json!({ "project": actual_project }),
        remaining_budget(timeout, started)?,
    )?;
    confirm_index_status(&status, provider_key, &actual_project, expected_root)?;
    Ok(actual_project)
}

fn remaining_budget(timeout: Duration, started: Instant) -> Result<Duration, McpError> {
    timeout
        .checked_sub(started.elapsed())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| McpError::Timeout {
            method: "tools/call index_repository confirmation".to_string(),
            timeout,
        })
}

fn confirmed_upsert_identity(
    result: &McpToolCallResult,
    provider_key: &str,
) -> Result<String, McpError> {
    if result.is_error {
        return Err(rebind_protocol(
            provider_key,
            "provider returned a tool error",
        ));
    }
    let object = response_object(result, provider_key, "upsert")?;
    let status = status(&object, provider_key, "upsert")?;
    if !matches!(
        status.as_str(),
        "ready"
            | "fresh"
            | "complete"
            | "completed"
            | "indexed"
            | "created"
            | "updated"
            | "upserted"
    ) {
        return Err(rebind_protocol(
            provider_key,
            "upsert response did not confirm a usable indexed state",
        ));
    }
    provider_identity(&object, provider_key, "upsert")
}

fn confirm_index_status(
    result: &McpToolCallResult,
    provider_key: &str,
    actual_project: &str,
    expected_root: &Path,
) -> Result<(), McpError> {
    if result.is_error {
        return Err(rebind_protocol(
            provider_key,
            "targeted confirmation returned a tool error",
        ));
    }
    let object = response_object(result, provider_key, "targeted confirmation")?;
    let confirmed_project = provider_identity(&object, provider_key, "targeted confirmation")?;
    if confirmed_project != actual_project {
        return Err(rebind_protocol(
            provider_key,
            "targeted confirmation identified a different provider project",
        ));
    }
    if status(&object, provider_key, "targeted confirmation")? != "ready" {
        return Err(rebind_protocol(
            provider_key,
            "targeted confirmation did not report ready status",
        ));
    }
    let root = object
        .get("root_path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|root| !root.is_empty())
        .ok_or_else(|| {
            rebind_protocol(
                provider_key,
                "targeted confirmation omitted a non-empty canonical `root_path`",
            )
        })?;
    if root != expected_root.display().to_string() {
        return Err(rebind_protocol(
            provider_key,
            "targeted confirmation did not match the active canonical checkout root",
        ));
    }
    Ok(())
}

fn response_object(
    result: &McpToolCallResult,
    provider_key: &str,
    response: &str,
) -> Result<Map<String, Value>, McpError> {
    if result.text.len() > MAX_INDEX_RESPONSE_BYTES {
        return Err(rebind_protocol(
            provider_key,
            &format!("{response} response exceeded 65536 bytes"),
        ));
    }
    let value: Value = serde_json::from_str(result.text.trim())
        .map_err(|_| rebind_protocol(provider_key, &format!("{response} response was not JSON")))?;
    value.as_object().cloned().ok_or_else(|| {
        rebind_protocol(
            provider_key,
            &format!("{response} response must be a JSON object"),
        )
    })
}

fn provider_identity(
    object: &Map<String, Value>,
    provider_key: &str,
    response: &str,
) -> Result<String, McpError> {
    let mut identity = None;
    for field in ["project", "id", "name"] {
        let Some(value) = object.get(field) else {
            continue;
        };
        let value = value
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty() && value.len() <= MAX_PROVIDER_IDENTITY_BYTES)
            .filter(|value| !value.chars().any(char::is_control))
            .filter(|value| !looks_like_filesystem_path(value))
            .ok_or_else(|| {
                rebind_protocol(
                    provider_key,
                    &format!(
                        "{response} field `{field}` must be a bounded non-empty non-path string"
                    ),
                )
            })?;
        match &identity {
            Some(previous) if previous != value => {
                return Err(rebind_protocol(
                    provider_key,
                    &format!("{response} reported conflicting provider identities"),
                ));
            }
            Some(_) => {}
            None => identity = Some(value.to_string()),
        }
    }
    identity.ok_or_else(|| {
        rebind_protocol(
            provider_key,
            &format!("{response} omitted the provider project identity"),
        )
    })
}

fn status(
    object: &Map<String, Value>,
    provider_key: &str,
    response: &str,
) -> Result<String, McpError> {
    let mut statuses = Vec::new();
    for field in ["status", "state"] {
        let Some(value) = object.get(field) else {
            continue;
        };
        let status = value
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                rebind_protocol(
                    provider_key,
                    &format!("{response} field `{field}` must be a non-empty string"),
                )
            })?;
        statuses.push(status.to_ascii_lowercase());
    }
    let Some(status) = statuses.first() else {
        return Err(rebind_protocol(
            provider_key,
            &format!("{response} omitted status"),
        ));
    };
    if statuses.iter().any(|candidate| candidate != status) {
        return Err(rebind_protocol(
            provider_key,
            &format!("{response} reported conflicting status/state values"),
        ));
    }
    Ok(status.clone())
}

fn looks_like_filesystem_path(value: &str) -> bool {
    let path = Path::new(value);
    path.is_absolute()
        || value.starts_with('~')
        || value.contains('\\')
        || value.as_bytes().get(1) == Some(&b':')
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
}

fn rebind_protocol(provider_key: &str, message: &str) -> McpError {
    McpError::Protocol(format!(
        "stable codebase-memory index upsert for `{provider_key}` was not confirmed: {message}"
    ))
}
