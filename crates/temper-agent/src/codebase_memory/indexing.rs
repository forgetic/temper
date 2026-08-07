use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use serde_json::{Map, Value, json};
use temper_protocol_agent::{CodebaseMemoryIndex, CodebaseMemoryMode, CodebaseMemoryToolConfig};

use crate::mcp::{
    McpError, McpToolCallResult, McpToolDescriptor, StdioMcpClient, StdioMcpServerConfig,
};
use temper_agent_core::AgentContainmentContext;

use super::advertised_tool;
use super::background::BackgroundIndex;
use super::scope::{ProjectIndexState, WorkspaceScope};

const MAX_INDEX_UPSERT_RESPONSE_BYTES: usize = 64 * 1024;

pub(super) async fn prepare_indexes(
    config: &CodebaseMemoryToolConfig,
    mcp_config: &StdioMcpServerConfig,
    advertised: &[McpToolDescriptor],
    scope: &mut WorkspaceScope,
    containment: &AgentContainmentContext,
) -> std::result::Result<Vec<String>, McpError> {
    let mut notes = Vec::new();
    if config.index == CodebaseMemoryIndex::Off {
        notes.push("index=off; no internal indexing was attempted".to_string());
        return Ok(notes);
    }

    let repo_indices = scope.projects_needing_index();
    if repo_indices.is_empty() {
        notes.push(
            "no prepared repo was confirmed missing or stale; no indexing was attempted"
                .to_string(),
        );
        return Ok(notes);
    }

    // The provider contract is validated before discovery. Keep this local
    // assertion defensive in case startup ordering changes later.
    if !advertised_tool(advertised, "index_repository") {
        return Err(McpError::Protocol(
            "stable codebase-memory indexing unavailable: missing index_repository".to_string(),
        ));
    }

    let timeout = Duration::from_secs(config.index_timeout_secs);
    for index in repo_indices {
        let path = scope.projects[index].root.clone();
        let provider_key = scope.projects[index].provider_key.clone();
        if config.index == CodebaseMemoryIndex::Background {
            match start_background_index_repository(
                mcp_config,
                path,
                provider_key,
                timeout,
                containment.clone(),
            ) {
                Ok(background) => {
                    scope.projects[index].index_state = ProjectIndexState::BackgroundInProgress;
                    scope.projects[index].background_index = Some(background);
                    notes.push(format!(
                        "stable index upsert started for prepared repo `{}` (background indexing may still be in progress)",
                        scope.projects[index].canonical_alias
                    ));
                }
                Err(message) if config.mode == CodebaseMemoryMode::Auto => {
                    scope.projects[index].index_state = ProjectIndexState::IndexFailed;
                    notes.push(format!(
                        "stable index upsert background start failed for prepared repo `{}`; no path-keyed fallback was attempted: {message}",
                        scope.projects[index].canonical_alias
                    ));
                }
                Err(message) => return Err(McpError::Protocol(message)),
            }
            continue;
        }

        let result =
            call_index_repository(mcp_config, &path, &provider_key, timeout, containment).await;
        match result.and_then(|result| confirm_stable_upsert(&result, &provider_key)) {
            Ok(()) => {
                scope.projects[index].index_state = ProjectIndexState::Fresh;
                notes.push(format!(
                    "stable index upsert completed for prepared repo `{}` (blocking indexing completed)",
                    scope.projects[index].canonical_alias
                ));
            }
            Err(error) if config.mode == CodebaseMemoryMode::Auto => {
                scope.projects[index].index_state = ProjectIndexState::IndexFailed;
                notes.push(format!(
                    "stable index upsert failed for prepared repo `{}`; no path-keyed fallback was attempted ({})",
                    scope.projects[index].canonical_alias,
                    safe_index_failure_kind(&error)
                ));
            }
            Err(error) => return Err(error),
        }
    }
    Ok(notes)
}

fn start_background_index_repository(
    mcp_config: &StdioMcpServerConfig,
    path: PathBuf,
    provider_key: String,
    timeout: Duration,
    containment: AgentContainmentContext,
) -> std::result::Result<BackgroundIndex, String> {
    let mcp_config = mcp_config.clone();
    let tracker = BackgroundIndex::new();
    let tracker_for_thread = tracker.clone();
    thread::Builder::new()
        .name("codebase-memory-index".to_string())
        .spawn(move || {
            let completion = run_background_index_repository(
                mcp_config,
                path,
                provider_key,
                timeout,
                containment,
            );
            match completion {
                Ok(actual_project) => tracker_for_thread.complete_success(Some(actual_project)),
                Err(message) => {
                    tracing::warn!(
                        target: "temper::agent",
                        "background codebase-memory stable index upsert failed: {message}"
                    );
                    tracker_for_thread.complete_error(message);
                }
            }
        })
        .map(|_| tracker)
        .map_err(|error| format!("spawn background index_repository worker: {error}"))
}

fn run_background_index_repository(
    mcp_config: StdioMcpServerConfig,
    path: PathBuf,
    provider_key: String,
    timeout: Duration,
    containment: AgentContainmentContext,
) -> std::result::Result<String, String> {
    let client = StdioMcpClient::connect_blocking_with_containment(mcp_config, containment)
        .map_err(|error| format!("stable index upsert {}", safe_index_failure_kind(&error)))?;
    let result = client
        .call_tool_blocking(
            "index_repository",
            stable_index_arguments(&path, &provider_key),
            timeout,
        )
        .map_err(|error| format!("stable index upsert {}", safe_index_failure_kind(&error)))?;
    if result.is_error {
        return Err("stable index upsert returned a provider error".to_string());
    }
    confirm_stable_upsert(&result, &provider_key)
        .map_err(|_| "stable index upsert confirmation failed".to_string())?;
    Ok(provider_key)
}

async fn call_index_repository(
    mcp_config: &StdioMcpServerConfig,
    path: &Path,
    provider_key: &str,
    timeout: Duration,
    containment: &AgentContainmentContext,
) -> std::result::Result<McpToolCallResult, McpError> {
    // Isolate a blocking/timeout indexing call from the read-only client exposed
    // to the model. The stable name makes retries and concurrent requests an
    // upsert of one logical provider project.
    let index_client =
        StdioMcpClient::connect_with_containment(mcp_config.clone(), containment.clone()).await?;
    index_client
        .call_tool(
            "index_repository",
            stable_index_arguments(path, provider_key),
            timeout,
        )
        .await
}

fn stable_index_arguments(path: &Path, provider_key: &str) -> serde_json::Value {
    json!({
        "repo_path": path.display().to_string(),
        "name": provider_key,
    })
}

/// A successful RPC is not enough to make a cold project model-visible. The
/// provider must explicitly confirm that it upserted the requested stable key;
/// otherwise a path-keyed response could appear successful while the following
/// graph request remains unusable.
fn confirm_stable_upsert(
    result: &McpToolCallResult,
    provider_key: &str,
) -> std::result::Result<(), McpError> {
    if result.is_error {
        return Err(stable_upsert_protocol(
            provider_key,
            "provider returned a tool error",
        ));
    }
    if result.text.len() > MAX_INDEX_UPSERT_RESPONSE_BYTES {
        return Err(stable_upsert_protocol(
            provider_key,
            "response exceeded 65536 bytes",
        ));
    }
    let value: Value = serde_json::from_str(result.text.trim())
        .map_err(|_| stable_upsert_protocol(provider_key, "response was not JSON"))?;
    let object = value
        .as_object()
        .ok_or_else(|| stable_upsert_protocol(provider_key, "response must be a JSON object"))?;
    validate_upsert_identity(object, provider_key)?;
    let status = upsert_status(object, provider_key)?;
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
        return Err(stable_upsert_protocol(
            provider_key,
            "response did not confirm a usable indexed state",
        ));
    }
    Ok(())
}

fn validate_upsert_identity(
    object: &Map<String, Value>,
    provider_key: &str,
) -> std::result::Result<(), McpError> {
    let mut found = false;
    for field in ["project", "id", "name"] {
        let Some(value) = object.get(field) else {
            continue;
        };
        found = true;
        let actual = value
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                stable_upsert_protocol(
                    provider_key,
                    &format!("response field `{field}` must be a non-empty string"),
                )
            })?;
        if actual != provider_key {
            return Err(stable_upsert_protocol(
                provider_key,
                "response identified a different provider project",
            ));
        }
    }
    if !found {
        return Err(stable_upsert_protocol(
            provider_key,
            "response omitted the stable provider project identity",
        ));
    }
    Ok(())
}

fn upsert_status(
    object: &Map<String, Value>,
    provider_key: &str,
) -> std::result::Result<String, McpError> {
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
                stable_upsert_protocol(
                    provider_key,
                    &format!("response field `{field}` must be a non-empty string"),
                )
            })?;
        statuses.push(status.to_ascii_lowercase());
    }
    let Some(status) = statuses.first() else {
        return Err(stable_upsert_protocol(
            provider_key,
            "response omitted status",
        ));
    };
    if statuses.iter().any(|candidate| candidate != status) {
        return Err(stable_upsert_protocol(
            provider_key,
            "response reported conflicting status/state values",
        ));
    }
    Ok(status.clone())
}

fn stable_upsert_protocol(provider_key: &str, message: &str) -> McpError {
    McpError::Protocol(format!(
        "stable codebase-memory index upsert for `{provider_key}` was not confirmed: {message}"
    ))
}

fn safe_index_failure_kind(error: &McpError) -> &'static str {
    match error {
        McpError::Timeout { .. } => "timed out",
        McpError::Cancelled { .. } => "was cancelled",
        McpError::Spawn { .. } => "could not start the indexing client",
        McpError::Io { .. } | McpError::ProcessExited { .. } => "lost the indexing client",
        McpError::Json { .. } | McpError::ProtocolOverflow { .. } | McpError::Protocol(_) => {
            "returned an invalid provider response"
        }
        McpError::Rpc { .. } => "returned a provider error",
    }
}

pub(super) fn index_setting(index: CodebaseMemoryIndex) -> &'static str {
    match index {
        CodebaseMemoryIndex::Off => "off",
        CodebaseMemoryIndex::Background => "background",
        CodebaseMemoryIndex::Blocking => "blocking",
    }
}
