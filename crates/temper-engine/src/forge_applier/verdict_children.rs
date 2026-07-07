// SPDX-License-Identifier: MPL-2.0

//! Binding a verdict's `create_issues` children into the execution context:
//! mapping each [`JobChild`] to a [`CreateIssuesChild`] (resolving cross-repo
//! targets) and stamping the deterministic content correlation key.

use temper_forge::{Forge, ItemNumber, Repository, RepositoryId};
use temper_protocol_worker::JobChild;
use temper_workflow::{
    ArtifactKindId, ArtifactTarget, CreateIssuesChild, WorkflowMetadata, parse_metadata_block,
    replace_metadata_block,
};

use temper_runner::workspace_content_key;

use crate::InFlightJob;
use crate::forge_applier::ForgeApplier;
use crate::forge_applier::verdict::{
    VerdictChildrenBinding, create_issues_effect_index, parse_child_target_repo,
};
use crate::workflow_meta::artifact_kind_child_create_labels;

impl<F: Forge + ?Sized> ForgeApplier<F> {
    pub(super) async fn bind_create_issues_children(
        &self,
        binding: VerdictChildrenBinding<'_>,
    ) -> bool {
        let Some(effect_index) = create_issues_effect_index(self.workflow.as_ref(), binding.routed)
        else {
            tracing::warn!(
                target: "temper_daemon",
                job_id = %binding.job.job_id,
                repo = %binding.job.repo,
                issue = %binding.number,
                routed = %binding.routed,
                children = binding.children.len(),
                "forge applier ignored verdict children without create_issues effect"
            );
            return true;
        };

        let source_target_branch = match parse_source_target_branch(
            binding.job,
            binding.number,
            binding.source_body,
        ) {
            Ok(target_branch) => target_branch,
            Err(error) => {
                tracing::warn!(
                    target: "temper_daemon",
                    job_id = %binding.job.job_id,
                    repo = %binding.job.repo,
                    issue = %binding.number,
                    %error,
                    "forge applier dropped verdict apply with malformed source workflow metadata"
                );
                return false;
            }
        };

        let mut mapped = Vec::with_capacity(binding.children.len());
        for child in binding.children {
            let Some(mapped_child) = self
                .map_job_child(
                    binding.job,
                    binding.repository_id,
                    binding.number,
                    source_target_branch.as_deref(),
                    child,
                )
                .await
            else {
                return false;
            };
            mapped.push(mapped_child);
        }

        let content_key = workspace_content_key(
            &ArtifactKindId::new(binding.artifact_kind),
            binding.routed,
            binding.number,
        );
        binding
            .context
            .set_create_issues_at(binding.routed.clone(), effect_index, mapped);
        binding.context.set_create_issues_correlation_key_at(
            binding.routed.clone(),
            effect_index,
            content_key,
        );
        true
    }

    async fn map_job_child(
        &self,
        job: &InFlightJob,
        source_repo: &RepositoryId,
        number: ItemNumber,
        source_target_branch: Option<&str>,
        child: JobChild,
    ) -> Option<CreateIssuesChild> {
        let metadata = match parse_metadata_block(&child.body) {
            Ok(metadata) => metadata,
            Err(error) => {
                tracing::warn!(
                    target: "temper_daemon",
                    job_id = %job.job_id,
                    repo = %job.repo,
                    issue = %number,
                    child_slug = %child.slug,
                    %error,
                    "forge applier dropped verdict apply with malformed child workflow metadata"
                );
                return None;
            }
        };
        let child_kind =
            self.resolve_child_artifact_kind(job, number, &child, metadata.as_ref())?;
        let Some(kind) = self.workflow.artifact_kind(&child_kind) else {
            tracing::warn!(
                target: "temper_daemon",
                job_id = %job.job_id,
                repo = %job.repo,
                issue = %number,
                child_slug = %child.slug,
                child_kind = %child_kind,
                "forge applier dropped verdict apply with unknown child artifact kind"
            );
            return None;
        };
        if kind.target != ArtifactTarget::Issue {
            tracing::warn!(
                target: "temper_daemon",
                job_id = %job.job_id,
                repo = %job.repo,
                issue = %number,
                child_slug = %child.slug,
                child_kind = %child_kind,
                child_target = %kind.target,
                "forge applier dropped verdict apply with non-issue child artifact kind"
            );
            return None;
        }

        let labels =
            artifact_kind_child_create_labels(self.workflow.as_ref(), &child_kind, &child.labels)?;
        let body = match child_body_with_workflow_metadata(
            &child.body,
            metadata,
            &child_kind,
            source_target_branch,
        ) {
            Ok(body) => body,
            Err(error) => {
                tracing::warn!(
                    target: "temper_daemon",
                    job_id = %job.job_id,
                    repo = %job.repo,
                    issue = %number,
                    child_slug = %child.slug,
                    child_kind = %child_kind,
                    %error,
                    "forge applier dropped verdict apply after child workflow metadata update failed"
                );
                return None;
            }
        };
        let mut mapped = CreateIssuesChild {
            slug: child.slug,
            title: child.title,
            body,
            labels,
            dependencies: child.depends_on,
            target_repo: None,
        };

        if let Some(target_repo) = child.target_repo {
            let repository = self
                .resolve_child_target_repository(job, source_repo, number, &target_repo)
                .await?;
            mapped = mapped.with_target_repo(repository.id);
        }

        Some(mapped)
    }

    fn resolve_child_artifact_kind(
        &self,
        job: &InFlightJob,
        number: ItemNumber,
        child: &JobChild,
        metadata: Option<&WorkflowMetadata>,
    ) -> Option<ArtifactKindId> {
        let explicit_kind = match child.kind.as_deref() {
            Some(raw) => {
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    tracing::warn!(
                        target: "temper_daemon",
                        job_id = %job.job_id,
                        repo = %job.repo,
                        issue = %number,
                        child_slug = %child.slug,
                        "forge applier dropped verdict apply with empty child artifact kind"
                    );
                    return None;
                }
                Some(ArtifactKindId::new(trimmed))
            }
            None => None,
        };
        let metadata_kind = metadata.and_then(|metadata| metadata.kind.as_ref());
        if let Some(explicit) = &explicit_kind {
            if let Some(existing) = metadata_kind {
                if explicit != existing {
                    tracing::warn!(
                        target: "temper_daemon",
                        job_id = %job.job_id,
                        repo = %job.repo,
                        issue = %number,
                        child_slug = %child.slug,
                        child_kind = %explicit,
                        metadata_kind = %existing,
                        "forge applier dropped verdict apply with conflicting child artifact kinds"
                    );
                    return None;
                }
            }
        }

        Some(
            explicit_kind
                .or_else(|| metadata_kind.cloned())
                .unwrap_or_else(|| ArtifactKindId::new("code")),
        )
    }

    async fn resolve_child_target_repository(
        &self,
        job: &InFlightJob,
        source_repo: &RepositoryId,
        number: ItemNumber,
        target_repo: &str,
    ) -> Option<Repository> {
        let Some(path) = parse_child_target_repo(target_repo) else {
            tracing::warn!(
                target: "temper_daemon",
                job_id = %job.job_id,
                repo = %job.repo,
                issue = %number,
                child_target_repo = %target_repo,
                "forge applier dropped verdict apply with malformed child target_repo"
            );
            return None;
        };

        match self.forge.get_repository_by_path(&path).await {
            Ok(Some(repository)) => Some(repository),
            Ok(None) => {
                tracing::warn!(
                    target: "temper_daemon",
                    job_id = %job.job_id,
                    repo = %job.repo,
                    issue = %number,
                    source_repo = %source_repo,
                    child_target_repo = %target_repo,
                    "forge applier dropped verdict apply with unknown child target_repo"
                );
                None
            }
            Err(error) => {
                tracing::error!(
                    target: "temper_daemon",
                    job_id = %job.job_id,
                    repo = %job.repo,
                    issue = %number,
                    source_repo = %source_repo,
                    child_target_repo = %target_repo,
                    %error,
                    "forge applier dropped verdict apply after child target_repo lookup failed"
                );
                None
            }
        }
    }
}

fn parse_source_target_branch(
    job: &InFlightJob,
    number: ItemNumber,
    source_body: &str,
) -> Result<Option<String>, String> {
    let metadata = parse_metadata_block(source_body).map_err(|error| error.to_string())?;
    let target_branch = metadata
        .and_then(|metadata| metadata.target_branch)
        .and_then(|branch| {
            let trimmed = branch.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        });
    tracing::trace!(
        target: "temper_daemon",
        job_id = %job.job_id,
        repo = %job.repo,
        issue = %number,
        has_source_target_branch = target_branch.is_some(),
        "forge applier parsed source workflow metadata for verdict children"
    );
    Ok(target_branch)
}

fn child_body_with_workflow_metadata(
    body: &str,
    metadata: Option<WorkflowMetadata>,
    kind: &ArtifactKindId,
    source_target_branch: Option<&str>,
) -> Result<String, String> {
    let mut metadata = metadata.unwrap_or_default();
    let mut changed = false;
    if metadata.kind.is_none() {
        metadata.kind = Some(kind.clone());
        changed = true;
    }
    if !has_non_empty_target_branch(&metadata) {
        if let Some(source_target_branch) = source_target_branch {
            metadata.target_branch = Some(source_target_branch.to_string());
            changed = true;
        }
    }
    if !changed {
        return Ok(body.to_string());
    }
    replace_metadata_block(body, &metadata).map_err(|error| error.to_string())
}

fn has_non_empty_target_branch(metadata: &WorkflowMetadata) -> bool {
    metadata
        .target_branch
        .as_deref()
        .is_some_and(|branch| !branch.trim().is_empty())
}
