use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use serde_json::{Map, Value, json};
use temper_protocol_agent::{CodebaseMemoryIndex, WorkspaceContext, WorkspaceRepository};
use tongs::model::{ContentBlock, TextContent};
use tongs::tools::ToolOutput;

use super::{AllowedCodebaseMemoryTool, index_setting};

#[path = "discovery.rs"]
mod discovery;
pub(super) use discovery::{IndexedProject, parse_indexed_projects};
use discovery::{alias_looks_like_filesystem_path, resolve_repo_root, validate_safe_model_paths};

#[derive(Clone, Debug)]
pub(super) struct WorkspaceScope {
    pub(super) projects: Vec<ScopedProject>,
    alias_to_index: BTreeMap<String, usize>,
    ambiguous_aliases: BTreeSet<String>,
}

impl WorkspaceScope {
    pub(super) fn from_context(
        context: &WorkspaceContext,
        cwd: &Path,
    ) -> std::result::Result<Self, String> {
        if context.repos.is_empty() {
            return Err("workspace context contains no prepared repositories".to_string());
        }
        let workspace_root = cwd
            .canonicalize()
            .map_err(|error| format!("canonicalize workspace cwd `{}`: {error}", cwd.display()))?;
        let single_repo = context.repos.len() == 1;
        let mut projects = Vec::with_capacity(context.repos.len());
        for (index, repo) in context.repos.iter().enumerate() {
            projects.push(ScopedProject::from_repo(
                repo,
                index == 0,
                single_repo,
                &workspace_root,
            )?);
        }
        let mut scope = Self {
            projects,
            alias_to_index: BTreeMap::new(),
            ambiguous_aliases: BTreeSet::new(),
        };
        scope.rebuild_alias_map();
        Ok(scope)
    }

    pub(super) fn apply_discovered_projects(
        &mut self,
        discovered: Vec<IndexedProject>,
        discovery_available: bool,
    ) {
        for project in &mut self.projects {
            let matched = discovered
                .iter()
                .find(|indexed| project.matches_indexed_project(indexed))
                .cloned();
            match matched {
                Some(indexed) => {
                    let stale = indexed.stale.unwrap_or(false);
                    project.apply_indexed_project(indexed);
                    project.index_state = if stale {
                        ProjectIndexState::Stale
                    } else {
                        ProjectIndexState::Fresh
                    };
                }
                None if discovery_available => {
                    project.index_state = ProjectIndexState::Missing;
                }
                None => {
                    project.index_state = ProjectIndexState::Unknown;
                }
            }
        }
        self.rebuild_alias_map();
    }

    pub(super) fn rebuild_alias_map(&mut self) {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for project in &self.projects {
            for alias in project.aliases() {
                *counts.entry(alias).or_default() += 1;
            }
        }

        self.alias_to_index.clear();
        self.ambiguous_aliases.clear();
        for (index, project) in self.projects.iter().enumerate() {
            for alias in project.aliases() {
                if counts.get(&alias).copied().unwrap_or_default() == 1 {
                    self.alias_to_index.insert(alias, index);
                } else {
                    self.ambiguous_aliases.insert(alias);
                }
            }
        }
    }

    pub(super) fn projects_needing_index(&self) -> Vec<usize> {
        self.projects
            .iter()
            .enumerate()
            .filter_map(|(index, project)| project.index_state.needs_index().then_some(index))
            .collect()
    }

    pub(super) fn display_project_list(&self, indices: &[usize]) -> String {
        indices
            .iter()
            .filter_map(|index| self.projects.get(*index))
            .map(|project| project.canonical_alias.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn primary(&self) -> &ScopedProject {
        self.projects
            .iter()
            .find(|project| project.primary)
            .expect("scope always contains primary project")
    }

    fn resolve_alias(&self, raw: &str) -> std::result::Result<&ScopedProject, String> {
        let alias = raw.trim();
        if alias.is_empty() {
            return Err("project/repo alias must not be empty".to_string());
        }
        if alias_looks_like_filesystem_path(alias) {
            return Err(format!(
                "filesystem paths are not accepted as codebase-memory project/repo aliases; use one of the prepared workspace aliases: {}",
                self.documented_aliases().join(", ")
            ));
        }
        if self.ambiguous_aliases.contains(alias) {
            return Err(format!(
                "project/repo alias `{alias}` is ambiguous in this workspace; use one of: {}",
                self.documented_aliases().join(", ")
            ));
        }
        let Some(index) = self.alias_to_index.get(alias).copied() else {
            return Err(format!(
                "unknown codebase-memory project/repo alias `{alias}`; use one of the prepared workspace aliases: {}",
                self.documented_aliases().join(", ")
            ));
        };
        Ok(&self.projects[index])
    }

    fn documented_aliases(&self) -> Vec<String> {
        self.projects
            .iter()
            .flat_map(ScopedProject::documented_aliases)
            .filter(|alias| !self.ambiguous_aliases.contains(alias))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub(super) fn prepare_tool_input(
        &self,
        mcp_name: &str,
        default_project_key: Option<&'static str>,
        input: Value,
    ) -> std::result::Result<Value, String> {
        let mut object = match input {
            Value::Null => Map::new(),
            Value::Object(object) => object,
            other => {
                return Err(format!(
                    "codebase-memory tool input must be a JSON object, got {}",
                    value_kind(&other)
                ));
            }
        };

        validate_safe_model_paths(&object)?;

        let project_index = self.selected_project_index(&object)?;
        let should_default = mcp_name != "list_projects";
        match project_index {
            Some(index) => {
                let project = &self.projects[index];
                let actual = Value::String(project.actual_project.clone());
                let had_project = object.remove("project").is_some();
                let had_repo = object.remove("repo").is_some();
                let target_key =
                    default_project_key.unwrap_or(if had_project { "project" } else { "repo" });
                if had_project || had_repo {
                    object.insert(target_key.to_string(), actual);
                }
            }
            None if should_default => {
                if let Some(key) = default_project_key {
                    object.insert(
                        key.to_string(),
                        Value::String(self.primary().actual_project.clone()),
                    );
                }
            }
            None => {}
        }

        Ok(Value::Object(object))
    }

    fn selected_project_index(
        &self,
        object: &Map<String, Value>,
    ) -> std::result::Result<Option<usize>, String> {
        let mut selected: Option<usize> = None;
        for key in ["project", "repo"] {
            let Some(value) = object.get(key) else {
                continue;
            };
            let Some(alias) = value.as_str() else {
                return Err(format!("`{key}` must be a string workspace project alias"));
            };
            let project = self.resolve_alias(alias)?;
            let index = self
                .projects
                .iter()
                .position(|candidate| std::ptr::eq(candidate, project))
                .expect("resolved project belongs to scope");
            match selected {
                Some(previous) if previous != index => {
                    return Err(
                        "`project` and `repo` refer to different workspace repositories"
                            .to_string(),
                    );
                }
                Some(_) => {}
                None => selected = Some(index),
            }
        }
        Ok(selected)
    }

    pub(super) fn prompt_status(&self, index: CodebaseMemoryIndex, notes: &[String]) -> String {
        let mut lines = Vec::new();
        lines.push(format!(
            "- Default project: `{}` (actual codebase-memory project `{}`)",
            self.primary().canonical_alias,
            self.primary().actual_project
        ));
        lines.push(format!(
            "- Project aliases accepted in `project`/`repo`: {}",
            self.documented_aliases()
                .into_iter()
                .map(|alias| format!("`{alias}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        lines.push("- Filesystem paths are never accepted as project/repo values; use only the workspace aliases above.".to_string());
        lines.push(format!(
            "- Index setting: `{}`. {}",
            index_setting(index),
            match index {
                CodebaseMemoryIndex::Off => "No internal indexing was attempted; results may be missing or stale.",
                CodebaseMemoryIndex::Background => "Indexing was started for missing/stale prepared repos and may still be in progress.",
                CodebaseMemoryIndex::Blocking => "Missing/stale prepared repos were indexed before exposing tools unless auto mode recorded a warning.",
            }
        ));
        for project in &self.projects {
            lines.push(format!(
                "- `{}` status: {} (actual `{}`)",
                project.canonical_alias,
                project.index_state.as_prompt_text(),
                project.actual_project
            ));
        }
        for note in notes {
            lines.push(format!("- Note: {note}"));
        }
        lines.join("\n")
    }

    pub(super) fn details_json(&self) -> Value {
        json!({
            "default_project": self.primary().canonical_alias,
            "projects": self.projects.iter().map(|project| project.details_json()).collect::<Vec<_>>(),
        })
    }

    pub(super) fn list_projects_output(&self) -> ToolOutput {
        let text = serde_json::to_string_pretty(&self.details_json())
            .unwrap_or_else(|_| "{\"projects\":[]}".to_string());
        ToolOutput {
            content: vec![ContentBlock::Text(TextContent {
                text,
                text_signature: None,
            })],
            details: Some(json!({
                "mcp_tool": "list_projects",
                "workspace_scoped": true,
                "source": "temper-agent",
            })),
            is_error: false,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct ScopedProject {
    primary: bool,
    pub(super) canonical_alias: String,
    name: String,
    repo_id: String,
    dir: String,
    pub(super) root: PathBuf,
    actual_project: String,
    pub(super) index_state: ProjectIndexState,
}

impl ScopedProject {
    fn from_repo(
        repo: &WorkspaceRepository,
        primary: bool,
        single_repo: bool,
        workspace_root: &Path,
    ) -> std::result::Result<Self, String> {
        let canonical_alias = format!("{}/{}", repo.owner, repo.name);
        let root = resolve_repo_root(repo, single_repo, workspace_root)?;
        let project = Self {
            primary,
            canonical_alias: canonical_alias.clone(),
            name: repo.name.clone(),
            repo_id: repo.id.clone(),
            dir: repo.dir.clone(),
            root,
            actual_project: canonical_alias,
            index_state: ProjectIndexState::Unknown,
        };
        Ok(project)
    }

    fn aliases(&self) -> BTreeSet<String> {
        self.documented_aliases()
    }

    fn documented_aliases(&self) -> BTreeSet<String> {
        let mut aliases = BTreeSet::new();
        aliases.insert(self.canonical_alias.clone());
        aliases.insert(self.name.clone());
        aliases.insert(self.repo_id.clone());
        if self.dir != "." {
            aliases.insert(self.dir.clone());
        }
        aliases
    }

    fn matches_indexed_project(&self, indexed: &IndexedProject) -> bool {
        if let Some(path) = &indexed.path {
            return path_equivalent(path, &self.root)
                || (!path.is_absolute()
                    && (path == Path::new(&self.dir) || path == Path::new(&self.name)));
        }
        for name in indexed.names() {
            if self.aliases().contains(&name) {
                return true;
            }
        }
        false
    }

    pub(super) fn apply_indexed_project(&mut self, indexed: IndexedProject) {
        if let Some(id) = indexed.id.filter(|id| !id.trim().is_empty()) {
            self.actual_project = id;
        } else if let Some(name) = indexed.name.filter(|name| !name.trim().is_empty()) {
            self.actual_project = name;
        }
    }

    pub(super) fn details_json(&self) -> Value {
        json!({
            "project": self.canonical_alias,
            "aliases": self.documented_aliases().into_iter().collect::<Vec<_>>(),
            "actual_project": self.actual_project,
            "index_status": self.index_state.as_prompt_text(),
            "primary": self.primary,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ProjectIndexState {
    Unknown,
    Missing,
    Stale,
    Fresh,
    BackgroundInProgress,
    IndexFailed,
}

impl ProjectIndexState {
    fn needs_index(self) -> bool {
        matches!(
            self,
            Self::Unknown | Self::Missing | Self::Stale | Self::IndexFailed
        )
    }

    fn as_prompt_text(self) -> &'static str {
        match self {
            Self::Unknown => "unknown (project discovery unavailable)",
            Self::Missing => "missing from codebase-memory index",
            Self::Stale => "stale according to codebase-memory project metadata",
            Self::Fresh => "fresh/non-stale",
            Self::BackgroundInProgress => "background indexing may still be in progress",
            Self::IndexFailed => "indexing failed; results may be stale or missing",
        }
    }
}

fn path_equivalent(left: &Path, right: &Path) -> bool {
    normalize_path(left) == normalize_path(right)
}

fn normalize_path(path: &Path) -> PathBuf {
    path.canonicalize()
        .unwrap_or_else(|_| lexical_normalize(path))
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
            Component::RootDir | Component::Prefix(_) => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

pub(super) fn default_project_key(mcp_name: &str, input_schema: &Value) -> Option<&'static str> {
    if mcp_name == "list_projects" {
        return None;
    }
    let properties = input_schema.get("properties").and_then(Value::as_object);
    if properties.is_some_and(|properties| properties.contains_key("repo")) {
        Some("repo")
    } else {
        Some("project")
    }
}

pub(super) fn scoped_parameters(
    input_schema: &Value,
    allowed: AllowedCodebaseMemoryTool,
    scope: &WorkspaceScope,
) -> Value {
    if allowed.mcp_name == "list_projects" {
        return json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false,
        });
    }

    let mut schema = input_schema.clone();
    if !schema.is_object() {
        schema = json!({ "type": "object", "properties": {} });
    }
    let aliases = scope.documented_aliases();
    let description = format!(
        "Workspace project alias. Omit to use the primary repo `{}`. Accepted aliases: {}. Filesystem paths are rejected.",
        scope.primary().canonical_alias,
        aliases.join(", ")
    );
    if let Some(object) = schema.as_object_mut() {
        object
            .entry("type".to_string())
            .or_insert_with(|| Value::String("object".to_string()));
        let properties = object
            .entry("properties".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(properties) = properties.as_object_mut() {
            properties.insert(
                "project".to_string(),
                json!({
                    "type": "string",
                    "description": description,
                    "enum": aliases,
                }),
            );
        }
    }
    schema
}

pub(super) fn description_for(
    allowed: AllowedCodebaseMemoryTool,
    server_description: &str,
    scope: &WorkspaceScope,
) -> String {
    let base = match server_description.trim() {
        "" => format!("Call codebase-memory MCP tool `{}`.", allowed.mcp_name),
        description => description.to_string(),
    };
    format!(
        "{base}\n\nWorkspace scoped: default project `{}`; accepted `project`/`repo` aliases: {}. Unknown aliases and filesystem paths are rejected.\n\nRead-only wrapper around codebase-memory MCP tool `{}`.",
        scope.primary().canonical_alias,
        scope.documented_aliases().join(", "),
        allowed.mcp_name
    )
}

fn value_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}
