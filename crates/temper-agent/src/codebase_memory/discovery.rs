use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use serde_json::{Map, Value};
use temper_protocol_agent::WorkspaceRepository;

#[derive(Clone, Debug, Default)]
pub(in crate::codebase_memory) struct IndexedProject {
    pub(in crate::codebase_memory) id: Option<String>,
    pub(in crate::codebase_memory) name: Option<String>,
    pub(in crate::codebase_memory) path: Option<PathBuf>,
    aliases: BTreeSet<String>,
    pub(in crate::codebase_memory) stale: Option<bool>,
}

impl IndexedProject {
    pub(super) fn names(&self) -> BTreeSet<String> {
        let mut names = self.aliases.clone();
        if let Some(id) = &self.id {
            names.insert(id.clone());
        }
        if let Some(name) = &self.name {
            names.insert(name.clone());
        }
        names
    }
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

pub(in crate::codebase_memory) fn parse_indexed_projects(text: &str) -> Vec<IndexedProject> {
    let Ok(value) = serde_json::from_str::<Value>(text.trim()) else {
        return Vec::new();
    };
    let mut projects = Vec::new();
    collect_indexed_projects(&value, &mut projects);
    projects
}

fn collect_indexed_projects(value: &Value, projects: &mut Vec<IndexedProject>) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_indexed_projects(item, projects);
            }
        }
        Value::Object(object) => {
            if let Some(project) = indexed_project_from_object(object) {
                projects.push(project);
                return;
            }
            for key in ["project", "projects", "items", "data", "result"] {
                if let Some(value) = object.get(key) {
                    collect_indexed_projects(value, projects);
                }
            }
        }
        _ => {}
    }
}

fn indexed_project_from_object(object: &Map<String, Value>) -> Option<IndexedProject> {
    let has_project_shape = [
        "id",
        "project_id",
        "projectId",
        "name",
        "project",
        "project_name",
        "path",
        "root",
        "repo_path",
        "repository_path",
        "root_path",
        "rootPath",
    ]
    .iter()
    .any(|key| object.contains_key(*key));
    if !has_project_shape {
        return None;
    }

    let id = string_field(object, &["id", "project_id", "projectId"]);
    let name = string_field(object, &["name", "project", "project_name", "projectName"]);
    let path = string_field(
        object,
        &[
            "path",
            "root",
            "repo_path",
            "repoPath",
            "repository_path",
            "repositoryPath",
            "root_path",
            "rootPath",
            "workspace_path",
            "workspacePath",
        ],
    )
    .map(PathBuf::from);
    let mut aliases = BTreeSet::new();
    for key in ["alias", "aliases", "repo", "repository", "display_name"] {
        collect_string_values(object.get(key), &mut aliases);
    }
    let stale = bool_field(
        object,
        &[
            "stale",
            "outdated",
            "needs_index",
            "needsIndex",
            "has_changes",
            "hasChanges",
        ],
    );

    Some(IndexedProject {
        id,
        name,
        path,
        aliases,
        stale,
    })
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

fn collect_string_values(value: Option<&Value>, values: &mut BTreeSet<String>) {
    match value {
        Some(Value::String(value)) => {
            let value = value.trim();
            if !value.is_empty() {
                values.insert(value.to_string());
            }
        }
        Some(Value::Array(items)) => {
            for item in items {
                collect_string_values(Some(item), values);
            }
        }
        _ => {}
    }
}
