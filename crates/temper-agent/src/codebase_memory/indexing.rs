use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use serde_json::json;
use temper_protocol_agent::{CodebaseMemoryIndex, CodebaseMemoryMode, CodebaseMemoryToolConfig};

use crate::mcp::{McpError, McpToolDescriptor, StdioMcpServerConfig};
use temper_agent_core::AgentContainmentContext;

use super::advertised_tool;
use super::background::BackgroundIndex;
use super::confirmation::{confirm_current_root_binding, confirm_current_root_binding_blocking};
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
    if config.index == CodebaseMemoryIndex::Off {
        for project in &scope.projects {
            emit_index(
                &project.canonical_alias,
                &project.provider_key,
                "off",
                IndexOutcome::Disabled,
                FailureCategory::None,
            );
        }
        notes.push("index=off; no internal indexing was attempted".to_string());
        return Ok(notes);
    }

    let repo_indices = scope.projects_requiring_current_root_binding();
    if repo_indices.is_empty() {
        for project in &scope.projects {
            emit_index(
                &project.canonical_alias,
                &project.provider_key,
                index_setting(config.index),
                IndexOutcome::SkippedDiscoveryUnknown,
                FailureCategory::None,
            );
        }
        notes.push(
            "no prepared repo completed targeted discovery; no current-checkout rebind was attempted"
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
        let logical = scope.projects[index].canonical_alias.clone();
        let requested_outcome = if scope.projects[index].index_state == ProjectIndexState::Fresh {
            IndexOutcome::RebindFresh
        } else {
            IndexOutcome::Requested
        };
        emit_index(
            &logical,
            &provider_key,
            index_setting(config.index),
            requested_outcome,
            FailureCategory::None,
        );
        if config.index == CodebaseMemoryIndex::Background {
            match start_background_index_repository(
                mcp_config,
                path,
                provider_key.clone(),
                logical.clone(),
                timeout,
                containment.clone(),
            ) {
                Ok(background) => {
                    scope.projects[index].background_index = Some(background);
                    emit_index(
                        &logical,
                        &provider_key,
                        index_setting(config.index),
                        IndexOutcome::Started,
                        FailureCategory::None,
                    );
                    notes.push(format!(
                        "stable current-checkout rebind started for prepared repo `{}` (background indexing may still be in progress)",
                        scope.projects[index].canonical_alias
                    ));
                }
                Err(message) if config.mode == CodebaseMemoryMode::Auto => {
                    scope.projects[index].index_state = ProjectIndexState::IndexFailed;
                    emit_index(
                        &logical,
                        &provider_key,
                        index_setting(config.index),
                        IndexOutcome::Failed,
                        FailureCategory::Internal,
                    );
                    notes.push(format!(
                        "stable current-checkout rebind background start failed for prepared repo `{}`; no path-keyed fallback was attempted: {message}",
                        scope.projects[index].canonical_alias
                    ));
                }
                Err(message) => return Err(McpError::Protocol(message)),
            }
            continue;
        }

        match confirm_current_root_binding(mcp_config, &path, &provider_key, timeout, containment)
            .await
        {
            Ok(actual_project) => {
                scope.projects[index].confirmed_project = Some(actual_project);
                scope.projects[index].index_state = ProjectIndexState::CurrentRootBound;
                emit_index(
                    &logical,
                    &provider_key,
                    index_setting(config.index),
                    IndexOutcome::Completed,
                    FailureCategory::None,
                );
                notes.push(format!(
                    "stable current-checkout rebind completed for prepared repo `{}` (blocking indexing completed)",
                    scope.projects[index].canonical_alias
                ));
            }
            Err(error) if config.mode == CodebaseMemoryMode::Auto => {
                scope.projects[index].index_state = ProjectIndexState::IndexFailed;
                emit_index(
                    &logical,
                    &provider_key,
                    index_setting(config.index),
                    IndexOutcome::Failed,
                    FailureCategory::from(&error),
                );
                notes.push(format!(
                    "stable current-checkout rebind failed for prepared repo `{}`; no path-keyed fallback was attempted ({})",
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
    logical: String,
    timeout: Duration,
    containment: AgentContainmentContext,
) -> std::result::Result<BackgroundIndex, String> {
    let mcp_config = mcp_config.clone();
    let tracker = BackgroundIndex::new(provider_key.clone());
    let tracker_for_thread = tracker.clone();
    thread::Builder::new()
        .name("codebase-memory-index".to_string())
        .spawn(move || {
            let completion = run_background_index_repository(
                mcp_config,
                path,
                provider_key.clone(),
                timeout,
                containment,
            );
            match completion {
                Ok(actual_project) => {
                    tracker_for_thread.complete_success(Some(actual_project));
                    emit_index(
                        &logical,
                        &provider_key,
                        "background",
                        IndexOutcome::Completed,
                        FailureCategory::None,
                    );
                }
                Err(message) => {
                    tracing::warn!(
                        target: "temper::agent",
                        "background codebase-memory stable index upsert failed: {message}"
                    );
                    tracker_for_thread.complete_error(message);
                    emit_index(
                        &logical,
                        &provider_key,
                        "background",
                        IndexOutcome::Failed,
                        FailureCategory::Provider,
                    );
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
    confirm_current_root_binding_blocking(mcp_config, &path, &provider_key, timeout, containment)
        .map_err(|error| format!("stable index upsert {}", safe_index_failure_kind(&error)))
}

pub(super) fn stable_index_arguments(path: &Path, provider_key: &str) -> serde_json::Value {
    json!({
        "repo_path": path.display().to_string(),
        "name": provider_key,
    })
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
