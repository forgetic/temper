// SPDX-License-Identifier: MPL-2.0

//! Exact-head validation binding projection into worker jobs.

use temper_protocol_worker::JobContext;
use temper_runner::{ScanError, WorkItem};
use temper_workflow::{ToolManifest, parse_metadata_block};

use super::invalid_workflow_scan;

pub(super) fn bind(
    workflow: &temper_workflow::ValidatedWorkflow,
    item: &WorkItem,
    tool: &ToolManifest,
    checkout: &str,
    context: &mut JobContext,
) -> Result<(), ScanError> {
    let matches = workflow
        .validation_bindings()
        .iter()
        .filter(|binding| {
            binding.role == item.role
                && binding.action.as_str() == tool.name
                && binding.target_artifact == item.kind
        })
        .collect::<Vec<_>>();
    let ([] | [_]) = matches.as_slice() else {
        return Err(invalid_workflow_scan(format!(
            "action `{}` has multiple matching validation bindings",
            tool.name
        )));
    };
    let Some(binding) = matches.first() else {
        return Ok(());
    };
    if checkout != "read_only" {
        return Err(invalid_workflow_scan(format!(
            "validation binding `{}` requires read_only checkout",
            binding.id
        )));
    }
    let source_branch = context
        .source_metadata
        .get("target_branch")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            invalid_workflow_scan(format!(
                "validation binding `{}` requires source target_branch metadata",
                binding.id
            ))
        })?
        .to_string();
    context.source_metadata.insert(
        "validation_binding_id".to_string(),
        binding.id.as_str().to_string(),
    );
    context.source_metadata.insert(
        "validation_idempotency_key".to_string(),
        binding.idempotency_key.clone(),
    );
    context.source_metadata.insert(
        "validation_source_branch".to_string(),
        source_branch.clone(),
    );
    let workspace = context.workspace.as_mut().ok_or_else(|| {
        invalid_workflow_scan(format!(
            "validation binding `{}` requires a workspace manifest",
            binding.id
        ))
    })?;
    for repository in &mut workspace.repos {
        repository.access = temper_protocol_worker::RepoAccess::ReadOnly;
        repository.branch_hint = None;
    }
    let primary = workspace.repos.first_mut().ok_or_else(|| {
        invalid_workflow_scan(format!(
            "validation binding `{}` requires a primary checkout",
            binding.id
        ))
    })?;
    primary.base_branch = source_branch;
    context.source_metadata.insert(
        "validation_plan".to_string(),
        format!(
            "{}#{}",
            context.repo,
            context.artifact.as_ref().map_or(0, |value| value.number)
        ),
    );
    let feature = context
        .artifact
        .as_ref()
        .and_then(|artifact| parse_metadata_block(&artifact.body).ok().flatten())
        .and_then(|metadata| metadata.parents.into_iter().next())
        .ok_or_else(|| {
            invalid_workflow_scan(format!(
                "validation binding `{}` requires a feature parent",
                binding.id
            ))
        })?;
    let repo = context.repo.as_str();
    context.source_metadata.insert(
        "validation_feature".to_string(),
        format!("{repo}#{}", feature.number.get()),
    );
    Ok(())
}
