use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use sha2::{Digest, Sha256};

use serde_json::{Map, Value, json};
use temper_protocol_agent::{CodebaseMemoryIndex, WorkspaceContext, WorkspaceRepository};
use tongs::model::{ContentBlock, TextContent};
use tongs::tools::ToolOutput;

use super::background::BackgroundIndex;
use super::indexing::index_setting;

#[path = "discovery.rs"]
mod discovery;
pub(super) use discovery::discover_workspace_projects;
use discovery::{
    TargetedProjectState, alias_looks_like_filesystem_path, resolve_repo_root,
    validate_safe_model_paths,
};

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

    pub(super) fn apply_targeted_discovery(&mut self, states: Vec<TargetedProjectState>) {
        debug_assert_eq!(states.len(), self.projects.len());
        for (project, state) in self.projects.iter_mut().zip(states) {
            project.index_state = match state {
                TargetedProjectState::Missing => ProjectIndexState::Missing,
                TargetedProjectState::Stale => ProjectIndexState::Stale,
                TargetedProjectState::Fresh => ProjectIndexState::Fresh,
            };
        }
    }

    pub(super) fn mark_discovery_unavailable(&mut self) {
        for project in &mut self.projects {
            project.index_state = ProjectIndexState::DiscoveryUnavailable;
        }
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

    pub(super) fn primary(&self) -> &ScopedProject {
        &self.projects[self.primary_index()]
    }

    fn primary_index(&self) -> usize {
        self.projects
            .iter()
            .position(|project| project.primary)
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

    pub(super) fn documented_aliases(&self) -> Vec<String> {
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
        wait_timeout: Duration,
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
        let default_index =
            (should_default && default_project_key.is_some()).then(|| self.primary_index());
        let target_index = project_index.or(default_index);
        if let Some(index) = target_index {
            self.projects[index].wait_for_background_index(wait_timeout)?;
        }

        match project_index {
            Some(index) => {
                let project = &self.projects[index];
                let actual = Value::String(project.actual_project());
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
                        Value::String(self.primary().actual_project()),
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
            self.primary().actual_project()
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
                project.index_state().as_prompt_text(),
                project.actual_project()
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

    pub(super) fn primary_root(&self) -> &Path {
        &self.primary().root
    }

    pub(super) fn primary_actual_project(&self) -> String {
        self.primary().actual_project()
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
    pub(super) provider_key: String,
    pub(super) git_head: Option<String>,
    pub(super) index_state: ProjectIndexState,
    pub(super) background_index: Option<BackgroundIndex>,
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
        let provider_key = provider_key_for_repo(repo);
        let git_head = current_git_head(&root);
        let project = Self {
            primary,
            canonical_alias: canonical_alias.clone(),
            name: repo.name.clone(),
            repo_id: repo.id.clone(),
            dir: repo.dir.clone(),
            root,
            provider_key,
            git_head,
            index_state: ProjectIndexState::DiscoveryUnavailable,
            background_index: None,
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

    fn actual_project(&self) -> String {
        self.background_index
            .as_ref()
            .and_then(BackgroundIndex::actual_project)
            .unwrap_or_else(|| self.provider_key.clone())
    }

    fn index_state(&self) -> ProjectIndexState {
        self.background_index
            .as_ref()
            .map(BackgroundIndex::index_state)
            .unwrap_or(self.index_state)
    }

    fn wait_for_background_index(&self, timeout: Duration) -> std::result::Result<(), String> {
        let Some(background) = &self.background_index else {
            return Ok(());
        };
        background.wait(timeout).map_err(|message| {
            format!(
                "codebase-memory project `{}` is not ready: {message}",
                self.canonical_alias
            )
        })?;
        if background.actual_project().is_none() {
            return Err(format!(
                "codebase-memory project `{}` is not ready: background stable upsert finished without confirming its provider key",
                self.canonical_alias
            ));
        }
        Ok(())
    }

    pub(super) fn details_json(&self) -> Value {
        json!({
            "project": self.canonical_alias,
            "aliases": self.documented_aliases().into_iter().collect::<Vec<_>>(),
            "actual_project": self.actual_project(),
            "index_status": self.index_state().as_prompt_text(),
            "primary": self.primary,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ProjectIndexState {
    DiscoveryUnavailable,
    Missing,
    Stale,
    Fresh,
    BackgroundInProgress,
    IndexFailed,
}

impl ProjectIndexState {
    fn needs_index(self) -> bool {
        matches!(self, Self::Missing | Self::Stale)
    }

    fn as_prompt_text(self) -> &'static str {
        match self {
            Self::DiscoveryUnavailable => "discovery unavailable; indexing was not attempted",
            Self::Missing => "missing from codebase-memory index",
            Self::Stale => "stale according to codebase-memory project metadata",
            Self::Fresh => "fresh/non-stale",
            Self::BackgroundInProgress => "background indexing may still be in progress",
            Self::IndexFailed => "indexing failed; results may be stale or missing",
        }
    }
}

fn current_git_head(root: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|head| head.trim().to_string())
        .filter(|head| !head.is_empty())
}

pub(super) fn provider_key_for_repo(repo: &WorkspaceRepository) -> String {
    let mut digest = Sha256::new();
    for (label, value) in [
        (b"id".as_slice(), repo.id.as_bytes()),
        (b"owner".as_slice(), repo.owner.as_bytes()),
        (b"name".as_slice(), repo.name.as_bytes()),
    ] {
        digest.update((label.len() as u64).to_be_bytes());
        digest.update(label);
        digest.update((value.len() as u64).to_be_bytes());
        digest.update(value);
    }
    format!("temper-v1-{:x}", digest.finalize())
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
