use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use serde_json::json;
use temper_protocol_agent::{CodebaseMemoryIndex, CodebaseMemoryMode, CodebaseMemoryToolConfig};

use crate::mcp::{
    McpError, McpToolCallResult, McpToolDescriptor, StdioMcpClient, StdioMcpServerConfig,
};
use temper_agent_core::AgentContainmentContext;

use super::advertised_tool;
use super::background::BackgroundIndex;
use super::lifecycle_observability::{FailureCategory, IndexOutcome, emit_index};
use super::scope::{ProjectIndexState, WorkspaceScope};

pub(super) async fn prepare_indexes(
    config: &CodebaseMemoryToolConfig,
    mcp_config: &StdioMcpServerConfig,
    advertised: &[McpToolDescriptor],
    scope: &mut WorkspaceScope,
    containment: &AgentContainmentContext,
) -> std::result::Result<Vec<String>, McpError> {
    let mut notes = Vec::new();
    let mode = index_setting(config.index);
    if config.index == CodebaseMemoryIndex::Off {
        for project in &scope.projects {
            let outcome = if project.index_state.is_discovery_unavailable() {
                IndexOutcome::SkippedDiscoveryUnknown
            } else {
                IndexOutcome::Disabled
            };
            emit_index(
                &project.canonical_alias,
                &project.provider_key,
                mode,
                outcome,
                FailureCategory::None,
            );
        }
        notes.push("index=off; no internal indexing was attempted".to_string());
        return Ok(notes);
    }

    for project in &scope.projects {
        if project.index_state.is_fresh() {
            emit_index(
                &project.canonical_alias,
                &project.provider_key,
                mode,
                IndexOutcome::Reused,
                FailureCategory::None,
            );
        } else if project.index_state.is_discovery_unavailable() {
            emit_index(
                &project.canonical_alias,
                &project.provider_key,
                mode,
                IndexOutcome::SkippedDiscoveryUnknown,
                FailureCategory::None,
            );
        }
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
    let mut requested_provider_keys = BTreeMap::<String, usize>::new();
    for index in repo_indices {
        let path = scope.projects[index].root.clone();
        let provider_key = scope.projects[index].provider_key.clone();
        let logical = scope.projects[index].canonical_alias.clone();
        emit_index(
            &logical,
            &provider_key,
            mode,
            IndexOutcome::Requested,
            FailureCategory::None,
        );

        if let Some(previous) = requested_provider_keys.get(&provider_key).copied() {
            scope.projects[index].index_state = scope.projects[previous].index_state;
            scope.projects[index].background_index =
                scope.projects[previous].background_index.clone();
            emit_index(
                &logical,
                &provider_key,
                mode,
                IndexOutcome::SuppressedDuplicate,
                FailureCategory::None,
            );
            notes.push(format!(
                "duplicate stable index request was suppressed for prepared repo `{logical}`"
            ));
            continue;
        }
        requested_provider_keys.insert(provider_key.clone(), index);

        if config.index == CodebaseMemoryIndex::Background {
            match start_background_index_repository(
                mcp_config,
                path,
                logical.clone(),
                provider_key.clone(),
                timeout,
                containment.clone(),
            ) {
                Ok(background) => {
                    scope.projects[index].index_state = ProjectIndexState::BackgroundInProgress;
                    scope.projects[index].background_index = Some(background);
                    notes.push(format!(
                        "stable index upsert started for prepared repo `{logical}` (background indexing may still be in progress)"
                    ));
                }
                Err(message) if config.mode == CodebaseMemoryMode::Auto => {
                    scope.projects[index].index_state = ProjectIndexState::IndexFailed;
                    emit_index(
                        &logical,
                        &provider_key,
                        mode,
                        IndexOutcome::Failed,
                        FailureCategory::Internal,
                    );
                    notes.push(format!(
                        "stable index upsert background start failed for prepared repo `{logical}`; no path-keyed fallback was attempted: {message}"
                    ));
                }
                Err(message) => {
                    emit_index(
                        &logical,
                        &provider_key,
                        mode,
                        IndexOutcome::Failed,
                        FailureCategory::Internal,
                    );
                    return Err(McpError::Protocol(message));
                }
            }
            continue;
        }

        emit_index(
            &logical,
            &provider_key,
            mode,
            IndexOutcome::Started,
            FailureCategory::None,
        );
        let result =
            call_index_repository(mcp_config, &path, &provider_key, timeout, containment).await;
        match result {
            Ok(result) if result.is_error => {
                let message = format!(
                    "stable index upsert reported an error for prepared repo `{logical}`: {}",
                    result.text
                );
                emit_index(
                    &logical,
                    &provider_key,
                    mode,
                    IndexOutcome::Failed,
                    FailureCategory::Provider,
                );
                if config.mode == CodebaseMemoryMode::Auto {
                    scope.projects[index].index_state = ProjectIndexState::IndexFailed;
                    notes.push(format!("{message}; no path-keyed fallback was attempted"));
                } else {
                    return Err(McpError::Rpc {
                        method: "tools/call index_repository".to_string(),
                        message,
                    });
                }
            }
            Ok(_) => {
                scope.projects[index].index_state = ProjectIndexState::Fresh;
                emit_index(
                    &logical,
                    &provider_key,
                    mode,
                    IndexOutcome::Completed,
                    FailureCategory::None,
                );
                notes.push(format!(
                    "stable index upsert completed for prepared repo `{logical}` (blocking indexing completed)"
                ));
            }
            Err(error) if config.mode == CodebaseMemoryMode::Auto => {
                scope.projects[index].index_state = ProjectIndexState::IndexFailed;
                emit_index(
                    &logical,
                    &provider_key,
                    mode,
                    IndexOutcome::Failed,
                    FailureCategory::from(&error),
                );
                notes.push(format!(
                    "stable index upsert failed for prepared repo `{logical}`; no path-keyed fallback was attempted: {error}"
                ));
            }
            Err(error) => {
                emit_index(
                    &logical,
                    &provider_key,
                    mode,
                    IndexOutcome::Failed,
                    FailureCategory::from(&error),
                );
                return Err(error);
            }
        }
    }
    Ok(notes)
}

fn start_background_index_repository(
    mcp_config: &StdioMcpServerConfig,
    path: PathBuf,
    logical: String,
    provider_key: String,
    timeout: Duration,
    containment: AgentContainmentContext,
) -> std::result::Result<BackgroundIndex, String> {
    let mcp_config = mcp_config.clone();
    let tracker = BackgroundIndex::new(provider_key.clone());
    let tracker_for_thread = tracker.clone();
    thread::Builder::new()
        .name("codebase-memory-index".to_string())
        .spawn(move || {
            emit_index(
                &logical,
                &provider_key,
                "background",
                IndexOutcome::Started,
                FailureCategory::None,
            );
            let completion = run_background_index_repository(
                mcp_config,
                path,
                provider_key.clone(),
                timeout,
                containment,
            );
            match completion {
                Ok(actual_project) => {
                    emit_index(
                        &logical,
                        &provider_key,
                        "background",
                        IndexOutcome::Completed,
                        FailureCategory::None,
                    );
                    tracker_for_thread.complete_success(Some(actual_project));
                }
                Err(message) => {
                    emit_index(
                        &logical,
                        &provider_key,
                        "background",
                        IndexOutcome::Failed,
                        FailureCategory::Provider,
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
        .map_err(|error| error.to_string())?;
    let result = client
        .call_tool_blocking(
            "index_repository",
            stable_index_arguments(&path, &provider_key),
            timeout,
        )
        .map_err(|error| error.to_string())?;
    if result.is_error {
        return Err(format!(
            "index_repository returned an error: {}",
            result.text
        ));
    }
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

pub(super) fn index_setting(index: CodebaseMemoryIndex) -> &'static str {
    match index {
        CodebaseMemoryIndex::Off => "off",
        CodebaseMemoryIndex::Background => "background",
        CodebaseMemoryIndex::Blocking => "blocking",
    }
}
