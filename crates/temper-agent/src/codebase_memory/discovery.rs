use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde_json::{Map, Value, json};
use temper_protocol_agent::WorkspaceRepository;

use crate::mcp::{McpError, McpToolCallResult, StdioMcpClient};

use super::WorkspaceScope;

const MAX_TARGETED_DISCOVERY_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::codebase_memory) enum TargetedProjectState {
    Missing,
    Stale,
    Fresh,
    Migrated,
}

pub(in crate::codebase_memory) struct TargetedDiscovery {
    pub states: Vec<TargetedProjectState>,
    pub cache_bytes: Option<u64>,
}

pub(in crate::codebase_memory) struct TargetedDiscoveryFailure {
    pub error: McpError,
    pub record_count: usize,
    pub cache_bytes: Option<u64>,
}

/// Looks up only the host-derived provider keys for this workspace. Results are
/// accumulated before they are applied so any timeout or malformed response
/// leaves every repository discovery-unavailable and cannot trigger a partial
/// indexing pass.
pub(in crate::codebase_memory) async fn discover_workspace_projects(
    client: &StdioMcpClient,
    timeout: Duration,
    scope: &WorkspaceScope,
) -> Result<TargetedDiscovery, TargetedDiscoveryFailure> {
    let mut states = Vec::with_capacity(scope.projects.len());
    let mut cache_bytes = None;
    let started = std::time::Instant::now();
    for project in &scope.projects {
        let remaining = timeout.checked_sub(started.elapsed()).ok_or_else(|| {
            discovery_failure(
                McpError::Timeout {
                    method: "tools/call index_status".to_string(),
                    timeout,
                },
                states.len(),
                cache_bytes,
            )
        })?;
        let result = client
            .call_tool(
                "index_status",
                json!({ "project": project.provider_key }),
                remaining,
            )
            .await
            .map_err(|error| discovery_failure(error, states.len(), cache_bytes))?;
        let (state, reported_cache_bytes) =
            parse_targeted_status(&result, &project.provider_key, project.git_head.as_deref())
                .map_err(|error| discovery_failure(error, states.len(), cache_bytes))?;
        states.push(state);
        if let Some(reported) = reported_cache_bytes {
            cache_bytes = Some(cache_bytes.map_or(reported, |known: u64| known.max(reported)));
        }
    }
    Ok(TargetedDiscovery {
        states,
        cache_bytes,
    })
}

fn discovery_failure(
    error: McpError,
    record_count: usize,
    cache_bytes: Option<u64>,
) -> TargetedDiscoveryFailure {
    TargetedDiscoveryFailure {
        error,
        record_count,
        cache_bytes,
    }
}

fn parse_targeted_status(
    result: &McpToolCallResult,
    provider_key: &str,
    checkout_head: Option<&str>,
) -> Result<(TargetedProjectState, Option<u64>), McpError> {
    if result.text.len() > MAX_TARGETED_DISCOVERY_BYTES {
        return Err(discovery_protocol_error(
            provider_key,
            "targeted response exceeded 65536 bytes",
        ));
    }
    let value: Value = serde_json::from_str(result.text.trim()).map_err(|error| {
        discovery_protocol_error(provider_key, &format!("response was not JSON: {error}"))
    })?;
    let object = value
        .as_object()
        .ok_or_else(|| discovery_protocol_error(provider_key, "response must be a JSON object"))?;
    let cache_bytes = u64_field(object, &["cache_bytes", "cacheBytes"]);

    if result.is_error {
        if response_is_missing(object) {
            return Ok((TargetedProjectState::Missing, cache_bytes));
        }
        return Err(discovery_protocol_error(
            provider_key,
            "provider returned a tool error without an explicit missing status",
        ));
    }
    if response_is_missing(object) {
        return Ok((TargetedProjectState::Missing, cache_bytes));
    }
    if let Some(actual) = string_field(object, &["project", "id", "name"]) {
        if actual != provider_key {
            return Err(discovery_protocol_error(
                provider_key,
                "response identified a different provider project",
            ));
        }
    }

    if bool_field(
        object,
        &[
            "stale",
            "outdated",
            "needs_index",
            "needsIndex",
            "has_changes",
            "hasChanges",
        ],
    ) == Some(true)
    {
        return Ok((TargetedProjectState::Stale, cache_bytes));
    }

    let status = string_field(object, &["status", "state"])
        .map(|status| status.to_ascii_lowercase())
        .ok_or_else(|| discovery_protocol_error(provider_key, "response omitted status/state"))?;
    if matches!(
        status.as_str(),
        "stale" | "outdated" | "dirty" | "changed" | "needs_index" | "needs-index"
    ) {
        return Ok((TargetedProjectState::Stale, cache_bytes));
    }
    if status == "migrated" {
        return Ok((TargetedProjectState::Migrated, cache_bytes));
    }
    if !matches!(
        status.as_str(),
        "ready" | "fresh" | "complete" | "completed" | "indexed"
    ) {
        return Err(discovery_protocol_error(
            provider_key,
            &format!("response reported unsupported status `{status}`"),
        ));
    }

    if let (Some(checkout_head), Some(indexed_head)) = (
        checkout_head,
        object
            .get("git")
            .and_then(Value::as_object)
            .and_then(|git| string_field(git, &["head_sha", "headSha", "commit"])),
    ) {
        if checkout_head != indexed_head {
            return Ok((TargetedProjectState::Stale, cache_bytes));
        }
    }
    Ok((TargetedProjectState::Fresh, cache_bytes))
}

fn u64_field(object: &Map<String, Value>, keys: &[&str]) -> Option<u64> {
    keys.iter()
        .find_map(|key| object.get(*key).and_then(Value::as_u64))
}

fn response_is_missing(object: &Map<String, Value>) -> bool {
    string_field(object, &["status", "state"]).is_some_and(|status| {
        matches!(
            status.to_ascii_lowercase().as_str(),
            "missing" | "not_found" | "not-found" | "not_indexed" | "not-indexed"
        )
    })
}

fn discovery_protocol_error(provider_key: &str, message: &str) -> McpError {
    McpError::Protocol(format!(
        "targeted codebase-memory discovery for `{provider_key}` failed: {message}"
    ))
}

pub(super) fn resolve_repo_root(
    repo: &WorkspaceRepository,
    single_repo: bool,
    workspace_root: &Path,
) -> std::result::Result<PathBuf, String> {
    let dir = repo.dir.trim();
    if dir.is_empty() {
        return Err(format!(
            "prepared repo `{}/{}` has an empty dir",
            repo.owner, repo.name
        ));
    }
    if dir == "." && !single_repo {
        return Err(format!(
            "prepared repo `{}/{}` uses dir `.` in a multi-repo workspace",
            repo.owner, repo.name
        ));
    }
    let dir_path = Path::new(dir);
    validate_safe_repo_dir(dir_path).map_err(|message| {
        format!(
            "prepared repo `{}/{}` has unsafe dir `{}`: {message}",
            repo.owner, repo.name, repo.dir
        )
    })?;

    let candidate = if dir == "." {
        workspace_root.to_path_buf()
    } else {
        workspace_root.join(dir_path)
    };
    let canonical_candidate = candidate.canonicalize();
    let root = match canonical_candidate {
        Ok(path) => path,
        Err(_error) if single_repo && cwd_looks_like_single_repo_checkout(workspace_root, repo) => {
            workspace_root.to_path_buf()
        }
        Err(error) => {
            return Err(format!(
                "prepared repo path `{}` does not resolve safely: {error}",
                candidate.display()
            ));
        }
    };

    if !root.starts_with(workspace_root) {
        return Err(format!(
            "prepared repo path `{}` escapes workspace root `{}`",
            root.display(),
            workspace_root.display()
        ));
    }
    Ok(root)
}

fn cwd_looks_like_single_repo_checkout(cwd: &Path, repo: &WorkspaceRepository) -> bool {
    cwd.join(".git").exists()
        || cwd
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == repo.dir || name == repo.name)
}

fn validate_safe_repo_dir(path: &Path) -> std::result::Result<(), &'static str> {
    if path.is_absolute() {
        return Err("absolute paths are not allowed");
    }
    let mut has_component = false;
    for component in path.components() {
        match component {
            Component::Normal(_) | Component::CurDir => has_component = true,
            Component::ParentDir => return Err("parent-directory components are not allowed"),
            Component::RootDir | Component::Prefix(_) => {
                return Err("root/prefix components are not allowed");
            }
        }
    }
    if !has_component {
        return Err("path must name the prepared checkout directory");
    }
    Ok(())
}

pub(super) fn alias_looks_like_filesystem_path(alias: &str) -> bool {
    let path = Path::new(alias);
    path.is_absolute()
        || alias.starts_with('~')
        || alias.contains('\\')
        || alias.as_bytes().get(1) == Some(&b':')
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
}

pub(super) fn validate_safe_model_paths(
    object: &Map<String, Value>,
) -> std::result::Result<(), String> {
    for (key, value) in object {
        validate_model_path_value(key, value)?;
    }
    Ok(())
}

fn validate_model_path_value(key: &str, value: &Value) -> std::result::Result<(), String> {
    if is_path_key(key) {
        validate_path_value(key, value)?;
    }
    match value {
        Value::Object(object) => validate_safe_model_paths(object),
        Value::Array(values) => {
            for value in values {
                validate_model_path_value(key, value)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn is_path_key(key: &str) -> bool {
    matches!(
        key,
        "path"
            | "paths"
            | "file"
            | "filePath"
            | "repo_path"
            | "repoPath"
            | "repository_path"
            | "repositoryPath"
            | "root"
            | "root_path"
            | "rootPath"
            | "dir"
            | "directory"
            | "workspace"
            | "workspace_path"
            | "workspacePath"
    )
}

fn validate_path_value(key: &str, value: &Value) -> std::result::Result<(), String> {
    match value {
        Value::String(path) => validate_relative_model_path(key, path),
        Value::Array(values) => {
            for value in values {
                validate_path_value(key, value)?;
            }
            Ok(())
        }
        Value::Null => Ok(()),
        _ => Ok(()),
    }
}

fn validate_relative_model_path(key: &str, path: &str) -> std::result::Result<(), String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    let path = Path::new(trimmed);
    if path.is_absolute()
        || trimmed.starts_with('~')
        || trimmed.contains('\\')
        || trimmed.as_bytes().get(1) == Some(&b':')
    {
        return Err(format!(
            "`{key}` must be a repository-relative path, not an absolute filesystem path"
        ));
    }
    for component in path.components() {
        match component {
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(format!(
                    "`{key}` must stay within the selected workspace repository"
                ));
            }
            Component::Normal(_) | Component::CurDir => {}
        }
    }
    Ok(())
}

fn string_field(object: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        object
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn bool_field(object: &Map<String, Value>, keys: &[&str]) -> Option<bool> {
    keys.iter().find_map(|key| match object.get(*key) {
        Some(Value::Bool(value)) => Some(*value),
        Some(Value::String(value)) => match value.trim().to_ascii_lowercase().as_str() {
            "true" | "yes" | "stale" | "dirty" | "changed" | "needs_index" => Some(true),
            "false" | "no" | "fresh" | "clean" => Some(false),
            _ => None,
        },
        _ => None,
    })
}
