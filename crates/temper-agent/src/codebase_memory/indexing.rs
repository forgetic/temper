use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use serde_json::json;
use temper_protocol_agent::{CodebaseMemoryIndex, CodebaseMemoryMode, CodebaseMemoryToolConfig};

use crate::mcp::{
    McpError, McpToolCallResult, McpToolDescriptor, StdioMcpClient, StdioMcpServerConfig,
};

use super::advertised_tool;
use super::scope::{ProjectIndexState, WorkspaceScope, parse_indexed_projects};

pub(super) async fn discover_indexed_projects(
    client: &StdioMcpClient,
    timeout: Duration,
) -> std::result::Result<Vec<super::scope::IndexedProject>, McpError> {
    let result = client
        .call_tool("list_projects", json!({}), timeout)
        .await?;
    Ok(parse_indexed_projects(&result.text))
}

pub(super) async fn prepare_indexes(
    config: &CodebaseMemoryToolConfig,
    client: &StdioMcpClient,
    mcp_config: &StdioMcpServerConfig,
    advertised: &[McpToolDescriptor],
    scope: &mut WorkspaceScope,
) -> std::result::Result<Vec<String>, McpError> {
    let mut notes = Vec::new();
    if config.index == CodebaseMemoryIndex::Off {
        notes.push("index=off; no internal indexing was attempted".to_string());
        return Ok(notes);
    }

    let repo_indices = scope.projects_needing_index();
    if repo_indices.is_empty() {
        notes.push("all prepared repos matched a non-stale codebase-memory project".to_string());
        return Ok(notes);
    }

    if !advertised_tool(advertised, "index_repository") {
        let message = format!(
            "index={}; codebase-memory MCP server did not advertise index_repository for prepared repos: {}",
            index_setting(config.index),
            scope.display_project_list(&repo_indices)
        );
        if config.mode == CodebaseMemoryMode::Auto {
            notes.push(message);
            return Ok(notes);
        }
        return Err(McpError::Protocol(message));
    }

    let timeout = Duration::from_secs(config.index_timeout_secs);
    for index in repo_indices {
        let path = scope.projects[index].root.clone();
        if config.index == CodebaseMemoryIndex::Background {
            match start_background_index_repository(mcp_config, path.clone(), timeout) {
                Ok(()) => {
                    scope.projects[index].index_state = ProjectIndexState::BackgroundInProgress;
                    notes.push(format!(
                        "index_repository started for prepared repo `{}` (background indexing may still be in progress)",
                        scope.projects[index].canonical_alias
                    ));
                }
                Err(message) if config.mode == CodebaseMemoryMode::Auto => {
                    scope.projects[index].index_state = ProjectIndexState::IndexFailed;
                    notes.push(format!(
                        "index_repository background start failed for prepared repo `{}`; continuing in auto mode with possibly stale tools: {message}",
                        scope.projects[index].canonical_alias
                    ));
                }
                Err(message) => return Err(McpError::Protocol(message)),
            }
            continue;
        }

        let result = call_index_repository(mcp_config, &path, timeout).await;
        match result {
            Ok(result) if result.is_error => {
                let message = format!(
                    "index_repository reported an error for prepared repo `{}`: {}",
                    scope.projects[index].canonical_alias, result.text
                );
                if config.mode == CodebaseMemoryMode::Auto {
                    scope.projects[index].index_state = ProjectIndexState::IndexFailed;
                    notes.push(message);
                } else {
                    return Err(McpError::Rpc {
                        method: "tools/call index_repository".to_string(),
                        message,
                    });
                }
            }
            Ok(result) => {
                let applied_actual_project = apply_index_repository_result(scope, index, &result);
                if !applied_actual_project && advertised_tool(advertised, "list_projects") {
                    refresh_actual_project_from_list(
                        config, client, timeout, scope, index, &mut notes,
                    )
                    .await?;
                }
                scope.projects[index].index_state = ProjectIndexState::Fresh;
                notes.push(format!(
                    "index_repository called for prepared repo `{}` (blocking indexing completed)",
                    scope.projects[index].canonical_alias
                ));
            }
            Err(error) if config.mode == CodebaseMemoryMode::Auto => {
                scope.projects[index].index_state = ProjectIndexState::IndexFailed;
                notes.push(format!(
                    "index_repository failed for prepared repo `{}`; continuing in auto mode with possibly stale tools: {error}",
                    scope.projects[index].canonical_alias
                ));
            }
            Err(error) => return Err(error),
        }
    }
    Ok(notes)
}

fn apply_index_repository_result(
    scope: &mut WorkspaceScope,
    index: usize,
    result: &McpToolCallResult,
) -> bool {
    parse_indexed_projects(&result.text)
        .into_iter()
        .next()
        .is_some_and(|project| scope.projects[index].apply_indexed_project(project))
}

async fn refresh_actual_project_from_list(
    config: &CodebaseMemoryToolConfig,
    client: &StdioMcpClient,
    timeout: Duration,
    scope: &mut WorkspaceScope,
    index: usize,
    notes: &mut Vec<String>,
) -> std::result::Result<(), McpError> {
    match discover_indexed_projects(client, timeout).await {
        Ok(discovered) => {
            if scope.apply_matching_discovered_project(index, discovered) {
                notes.push(format!(
                    "rediscovered codebase-memory project identity for prepared repo `{}` after indexing",
                    scope.projects[index].canonical_alias
                ));
            }
            Ok(())
        }
        Err(error) if config.mode == CodebaseMemoryMode::Auto => {
            notes.push(format!(
                "could not rediscover codebase-memory project identity for prepared repo `{}` after indexing; continuing in auto mode: {error}",
                scope.projects[index].canonical_alias
            ));
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn start_background_index_repository(
    mcp_config: &StdioMcpServerConfig,
    path: PathBuf,
    timeout: Duration,
) -> std::result::Result<(), String> {
    let mcp_config = mcp_config.clone();
    let repo_path = path.display().to_string();
    thread::Builder::new()
        .name("codebase-memory-index".to_string())
        .spawn(move || {
            let result = StdioMcpClient::connect_blocking(mcp_config).and_then(|client| {
                client.call_tool_blocking(
                    "index_repository",
                    json!({ "repo_path": repo_path }),
                    timeout,
                )
            });
            match result {
                Ok(result) if result.is_error => {
                    tracing::warn!(
                        target: "temper::agent",
                        "background codebase-memory index_repository returned an error: {}",
                        result.text
                    );
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(
                        target: "temper::agent",
                        "background codebase-memory index_repository failed: {error}"
                    );
                }
            }
        })
        .map(|_| ())
        .map_err(|error| format!("spawn background index_repository worker: {error}"))
}

async fn call_index_repository(
    mcp_config: &StdioMcpServerConfig,
    path: &Path,
    timeout: Duration,
) -> std::result::Result<McpToolCallResult, McpError> {
    // Use a short-lived MCP process for indexing so a blocking/timeout indexing
    // call cannot kill the long-lived client whose read-only tools are exposed
    // to the model.
    let index_client = StdioMcpClient::connect(mcp_config.clone()).await?;
    index_client
        .call_tool(
            "index_repository",
            json!({ "repo_path": path.display().to_string() }),
            timeout,
        )
        .await
}

pub(super) fn index_setting(index: CodebaseMemoryIndex) -> &'static str {
    match index {
        CodebaseMemoryIndex::Off => "off",
        CodebaseMemoryIndex::Background => "background",
        CodebaseMemoryIndex::Blocking => "blocking",
    }
}
