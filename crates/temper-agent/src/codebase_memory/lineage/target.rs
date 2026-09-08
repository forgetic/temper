//! Opaque source/read/mutation target admission for one agent run.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::Value;
use temper_agent_core::{
    EligibleWorkspaceTarget, InvocationTargetAdmission, MissingWorkspaceTarget,
    TargetAdmissionOutcome, TargetAdmissionStatus,
};
use temper_protocol_activity::{DecisionAnchorLineageV1, DecisionEvidenceKindV1};
use uuid::Uuid;

use super::super::scope::WorkspaceScope;
use crate::mcp::McpToolResultPart;
use crate::workspace_patch::{PatchOperation, patch_targets, verify_missing_target};

mod process;

const MAX_WORKSPACE_TARGET_IDENTITIES: usize = 64;

#[derive(Default)]
pub(super) struct WorkspaceTargetRegistry {
    identities: BTreeMap<PathBuf, EligibleWorkspaceTarget>,
    sources: BTreeMap<SourceTargetKey, TargetAdmissionOutcome>,
}

#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
struct SourceTargetKey {
    root_binding: String,
    evidence_kind: u8,
}

impl SourceTargetKey {
    fn new(lineage: &DecisionAnchorLineageV1) -> Option<Self> {
        Some(Self {
            root_binding: lineage.root_binding.clone(),
            evidence_kind: match lineage.decision_evidence_kind? {
                DecisionEvidenceKindV1::Implementation => 0,
                DecisionEvidenceKindV1::Caller => 1,
                DecisionEvidenceKindV1::FocusedTest => 2,
            },
        })
    }
}

impl WorkspaceTargetRegistry {
    pub(super) fn record_source(
        &mut self,
        scope: &WorkspaceScope,
        lineage: &DecisionAnchorLineageV1,
        input: &Value,
        typed_parts: Option<&[McpToolResultPart]>,
    ) {
        let Some(key) = SourceTargetKey::new(lineage) else {
            return;
        };
        let outcome = source_path(typed_parts)
            .and_then(|path| canonical_source_path(scope, input, &path))
            .map(|path| self.identity_for_source(path))
            .unwrap_or_else(TargetAdmissionOutcome::Ineligible);
        if lineage.implementation_authority_corrected {
            self.sources.insert(key, outcome);
            return;
        }
        match self.sources.get(&key) {
            None => {
                self.sources.insert(key, outcome);
            }
            Some(existing) if existing == &outcome => {}
            Some(_) => {
                self.sources.insert(
                    key,
                    TargetAdmissionOutcome::Ineligible(TargetAdmissionStatus::AmbiguousTarget),
                );
            }
        }
    }

    pub(super) fn resolve_source(
        &self,
        lineage: &DecisionAnchorLineageV1,
    ) -> TargetAdmissionOutcome {
        SourceTargetKey::new(lineage)
            .and_then(|key| self.sources.get(&key).cloned())
            .unwrap_or(TargetAdmissionOutcome::Ineligible(
                TargetAdmissionStatus::UnknownTarget,
            ))
    }

    pub(super) fn resolve_invocation(
        &mut self,
        scope: &WorkspaceScope,
        tool_name: &str,
        arguments: &Value,
    ) -> InvocationTargetAdmission {
        let object = match arguments.as_object() {
            Some(object) => object,
            None => {
                return InvocationTargetAdmission::Ineligible(
                    TargetAdmissionStatus::MalformedTarget,
                );
            }
        };
        match tool_name {
            "read" => {
                InvocationTargetAdmission::Read(self.resolve_read_path_argument(scope, object))
            }
            "write" | "edit" => {
                InvocationTargetAdmission::Mutation(vec![self.resolve_path_argument(scope, object)])
            }
            "apply_patch" => {
                let Some(patch) = object.get("patch").and_then(Value::as_str) else {
                    return InvocationTargetAdmission::Ineligible(
                        TargetAdmissionStatus::MalformedTarget,
                    );
                };
                self.resolve_patch(scope, patch)
            }
            "bash" => process::classify_bash(object),
            "submit_for_pr" => InvocationTargetAdmission::ControlPlane,
            _ => InvocationTargetAdmission::Ineligible(TargetAdmissionStatus::UnsupportedTool),
        }
    }

    fn resolve_patch(&self, scope: &WorkspaceScope, patch: &str) -> InvocationTargetAdmission {
        let targets = match patch_targets(patch) {
            Ok(targets) => targets,
            Err(status) => return InvocationTargetAdmission::Ineligible(status),
        };
        let mut existing = Vec::new();
        let mut creations = Vec::new();
        for (path, operation) in targets {
            match operation {
                PatchOperation::Existing => {
                    existing.push(self.resolve_workspace_path(scope, &path))
                }
                PatchOperation::Create => {
                    if let Err(status) = verify_missing_target(&scope.workspace_root, &path) {
                        return InvocationTargetAdmission::Ineligible(status);
                    }
                    creations.push(
                        MissingWorkspaceTarget::new(Uuid::new_v4().to_string())
                            .expect("v4 UUID is a valid run-local absence proof"),
                    );
                }
            }
        }
        if creations.is_empty() {
            InvocationTargetAdmission::Mutation(existing)
        } else {
            InvocationTargetAdmission::PatchCreation {
                existing,
                creations,
            }
        }
    }

    fn identity_for_source(&mut self, path: PathBuf) -> TargetAdmissionOutcome {
        if let Some(identity) = self.identities.get(&path) {
            return TargetAdmissionOutcome::Eligible(identity.clone());
        }
        if self.identities.len() >= MAX_WORKSPACE_TARGET_IDENTITIES {
            return TargetAdmissionOutcome::Ineligible(TargetAdmissionStatus::UnknownTarget);
        }
        let identity = EligibleWorkspaceTarget::new(Uuid::new_v4().to_string())
            .expect("v4 UUID is a valid opaque workspace target");
        self.identities.insert(path, identity.clone());
        TargetAdmissionOutcome::Eligible(identity)
    }

    fn resolve_read_path_argument(
        &mut self,
        scope: &WorkspaceScope,
        object: &serde_json::Map<String, Value>,
    ) -> TargetAdmissionOutcome {
        let Some(path) = object.get("path").and_then(Value::as_str) else {
            return TargetAdmissionOutcome::Ineligible(TargetAdmissionStatus::MalformedTarget);
        };
        match canonical_existing_path(&scope.workspace_root, &scope.workspace_root, path) {
            Ok(path) => self.identity_for_source(path),
            Err(status) => TargetAdmissionOutcome::Ineligible(status),
        }
    }

    fn resolve_path_argument(
        &self,
        scope: &WorkspaceScope,
        object: &serde_json::Map<String, Value>,
    ) -> TargetAdmissionOutcome {
        match object.get("path").and_then(Value::as_str) {
            Some(path) => self.resolve_workspace_path(scope, path),
            None => TargetAdmissionOutcome::Ineligible(TargetAdmissionStatus::MalformedTarget),
        }
    }

    fn resolve_workspace_path(&self, scope: &WorkspaceScope, path: &str) -> TargetAdmissionOutcome {
        canonical_existing_path(&scope.workspace_root, &scope.workspace_root, path)
            .map_err(TargetAdmissionOutcome::Ineligible)
            .and_then(|path| {
                self.identities
                    .get(&path)
                    .cloned()
                    .map(TargetAdmissionOutcome::Eligible)
                    .ok_or(TargetAdmissionOutcome::Ineligible(
                        TargetAdmissionStatus::UnknownTarget,
                    ))
            })
            .unwrap_or_else(|outcome| outcome)
    }
}

fn source_path(typed_parts: Option<&[McpToolResultPart]>) -> Result<String, TargetAdmissionStatus> {
    let mut paths = BTreeSet::new();
    for part in typed_parts.ok_or(TargetAdmissionStatus::MalformedTarget)? {
        let value = match part {
            McpToolResultPart::StructuredContent(value) => value.clone(),
            McpToolResultPart::Content(block) => {
                let object = block
                    .as_object()
                    .ok_or(TargetAdmissionStatus::MalformedTarget)?;
                if object.get("type").and_then(Value::as_str) != Some("text") {
                    continue;
                }
                serde_json::from_str(
                    object
                        .get("text")
                        .and_then(Value::as_str)
                        .ok_or(TargetAdmissionStatus::MalformedTarget)?,
                )
                .map_err(|_| TargetAdmissionStatus::MalformedTarget)?
            }
        };
        collect_source_paths(&value, &mut paths)?;
    }
    if paths.len() > 1 {
        return Err(TargetAdmissionStatus::AmbiguousTarget);
    }
    paths
        .pop_first()
        .ok_or(TargetAdmissionStatus::UnknownTarget)
}

fn collect_source_paths(
    value: &Value,
    paths: &mut BTreeSet<String>,
) -> Result<(), TargetAdmissionStatus> {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_source_paths(value, paths)?;
            }
        }
        Value::Object(object) => {
            if object.contains_key("source") {
                if object.get("source").and_then(Value::as_str).is_none() {
                    return Err(TargetAdmissionStatus::MalformedTarget);
                }
                let mut record_paths = BTreeSet::new();
                for field in ["file_path", "filePath", "path"] {
                    if let Some(value) = object.get(field) {
                        let value = value
                            .as_str()
                            .ok_or(TargetAdmissionStatus::MalformedTarget)?;
                        if value.trim().is_empty() {
                            return Err(TargetAdmissionStatus::MalformedTarget);
                        }
                        record_paths.insert(value.to_string());
                    }
                }
                if record_paths.len() > 1 {
                    return Err(TargetAdmissionStatus::CompetingTargets);
                }
                paths.extend(record_paths);
            }
            for field in ["results", "semantic_results", "semanticResults"] {
                if let Some(value) = object.get(field) {
                    collect_source_paths(value, paths)?;
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {
            return Err(TargetAdmissionStatus::MalformedTarget);
        }
    }
    Ok(())
}

fn canonical_source_path(
    scope: &WorkspaceScope,
    input: &Value,
    raw: &str,
) -> Result<PathBuf, TargetAdmissionStatus> {
    let project = selected_project(scope, input)?;
    let raw_path = Path::new(raw);
    let bases = if raw_path.is_absolute() {
        vec![project.root.as_path()]
    } else {
        vec![project.root.as_path(), scope.workspace_root.as_path()]
    };
    let mut paths = BTreeSet::new();
    let mut outside = false;
    for base in bases {
        match canonical_existing_path(&scope.workspace_root, base, raw) {
            Ok(path) if path.starts_with(&project.root) => {
                paths.insert(path);
            }
            Ok(_) | Err(TargetAdmissionStatus::OutsideWorkspace) => outside = true,
            Err(_) => {}
        }
    }
    match paths.len() {
        1 => Ok(paths.pop_first().expect("one canonical source path")),
        2.. => Err(TargetAdmissionStatus::AmbiguousTarget),
        _ if outside => Err(TargetAdmissionStatus::OutsideWorkspace),
        _ => Err(TargetAdmissionStatus::UnknownTarget),
    }
}

fn selected_project<'a>(
    scope: &'a WorkspaceScope,
    input: &Value,
) -> Result<&'a super::super::scope::ScopedProject, TargetAdmissionStatus> {
    let object = input
        .as_object()
        .ok_or(TargetAdmissionStatus::MalformedTarget)?;
    let selected = ["project", "repo"]
        .into_iter()
        .filter_map(|field| object.get(field))
        .map(|value| value.as_str().ok_or(TargetAdmissionStatus::MalformedTarget))
        .collect::<Result<Vec<_>, _>>()?;
    if selected.len() > 1 && selected[0] != selected[1] {
        return Err(TargetAdmissionStatus::CompetingTargets);
    }
    let Some(selected) = selected.first() else {
        return Ok(scope.primary());
    };
    let matches = scope
        .projects
        .iter()
        .filter(|project| project.actual_project() == *selected)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [project] => Ok(*project),
        [] => Err(TargetAdmissionStatus::UnknownTarget),
        _ => Err(TargetAdmissionStatus::AmbiguousTarget),
    }
}

fn canonical_existing_path(
    workspace_root: &Path,
    base: &Path,
    raw: &str,
) -> Result<PathBuf, TargetAdmissionStatus> {
    if raw.is_empty() || raw.trim() != raw || raw.contains('\0') {
        return Err(TargetAdmissionStatus::MalformedTarget);
    }
    let raw_path = Path::new(raw);
    let candidate = if raw_path.is_absolute() {
        raw_path.to_path_buf()
    } else {
        base.join(raw_path)
    };
    let canonical = candidate
        .canonicalize()
        .map_err(|_| TargetAdmissionStatus::UnknownTarget)?;
    if !canonical.starts_with(workspace_root) {
        return Err(TargetAdmissionStatus::OutsideWorkspace);
    }
    if !canonical.is_file() {
        return Err(TargetAdmissionStatus::UnknownTarget);
    }
    Ok(canonical)
}
