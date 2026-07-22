// SPDX-License-Identifier: MPL-2.0

//! Binding a verdict's `create_issues` children into the execution context:
//! mapping each [`JobChild`] to a [`CreateIssuesChild`] (resolving cross-repo
//! targets) and stamping the deterministic content correlation key.

use temper_forge::{Forge, ItemNumber, Repository, RepositoryId};
use temper_protocol_worker::JobChild;
use temper_workflow::{
    ArtifactKindId, ArtifactTarget, CreateIssuesChild, TargetBranchPolicy, TransitionId,
    ValidatedWorkflow, WorkflowMetadata, parse_metadata_block, replace_metadata_block,
};

use temper_runner::workspace_content_key;

use crate::InFlightJob;
use crate::forge_applier::ForgeApplier;
use crate::forge_applier::verdict::{
    VerdictChildrenBinding, create_issues_effect_index, parse_child_target_repo,
};
use crate::forge_applier::verdict_child_relations::{
    source_parent_kinds_after_transition, validate_child_parent_relation,
};
use crate::verdict_contract::{
    BranchResolutionContext, resolve_target_branch_requirement, source_metadata_from_workflow,
};
use crate::workflow_meta::artifact_kind_child_create_labels;

#[derive(Clone, Copy)]
struct ChildBranchBinding<'a> {
    legacy_source: Option<&'a str>,
    enforced: Option<&'a str>,
}

fn create_issues_target_branch_policy(
    workflow: &ValidatedWorkflow,
    transition: &TransitionId,
) -> Option<TargetBranchPolicy> {
    workflow
        .transitions()
        .iter()
        .find(|candidate| &candidate.id == transition)?
        .effects
        .iter()
        .find_map(|effect| match effect {
            temper_workflow::Effect::CreateIssues {
                target_branch_policy,
                ..
            } => *target_branch_policy,
            _ => None,
        })
}

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

        let source_workflow = match parse_source_workflow_metadata(
            binding.job,
            binding.number,
            binding.source_body,
        ) {
            Ok(metadata) => metadata,
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
        let source_target_branch = source_workflow
            .target_branch
            .as_deref()
            .map(str::trim)
            .filter(|branch| !branch.is_empty());
        let correlation_key = serde_json::from_value::<temper_protocol_worker::JobContext>(
            binding.job.job_payload.clone(),
        )
        .ok()
        .and_then(|context| {
            context
                .workspace
                .map(|workspace| workspace.coordination_key)
        });
        let enforced_target_branch = match create_issues_target_branch_policy(
            self.workflow.as_ref(),
            binding.routed,
        ) {
            Some(policy) => {
                let source_metadata = source_metadata_from_workflow(source_workflow.clone());
                match resolve_target_branch_requirement(
                    policy,
                    &BranchResolutionContext {
                        source_kind: binding.artifact_kind,
                        source_number: Some(binding.number.get()),
                        source_metadata: &source_metadata,
                        repository_default: &binding.repository.default_branch,
                        correlation_key: correlation_key.as_deref(),
                    },
                ) {
                    Ok(requirement) => Some(requirement.expected),
                    Err(error) => {
                        tracing::warn!(
                            target: "temper_daemon",
                            job_id = %binding.job.job_id,
                            repo = %binding.job.repo,
                            issue = %binding.number,
                            %error,
                            "forge applier dropped verdict apply with unresolved child target branch"
                        );
                        return false;
                    }
                }
            }
            None => None,
        };

        let source_kind = ArtifactKindId::new(binding.artifact_kind);
        let source_parent_kinds = source_parent_kinds_after_transition(
            self.workflow.as_ref(),
            &source_kind,
            binding.source_labels,
            binding.routed,
        );

        let mut mapped = Vec::with_capacity(binding.children.len());
        for child in binding.children {
            let Some(mapped_child) = self
                .map_job_child(
                    binding.job,
                    &binding.repository.id,
                    binding.number,
                    ChildBranchBinding {
                        legacy_source: source_target_branch,
                        enforced: enforced_target_branch.as_deref(),
                    },
                    &source_parent_kinds,
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
        branch: ChildBranchBinding<'_>,
        source_parent_kinds: &std::collections::BTreeSet<ArtifactKindId>,
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
        if !validate_child_parent_relation(
            self.workflow.as_ref(),
            job,
            number,
            &child,
            &child_kind,
            source_parent_kinds,
        ) {
            return None;
        }

        let labels =
            artifact_kind_child_create_labels(self.workflow.as_ref(), &child_kind, &child.labels)?;
        let body = match child_body_with_workflow_metadata(
            &child.body,
            metadata,
            &child_kind,
            branch,
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

fn parse_source_workflow_metadata(
    job: &InFlightJob,
    number: ItemNumber,
    source_body: &str,
) -> Result<WorkflowMetadata, String> {
    let metadata = parse_metadata_block(source_body)
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    tracing::trace!(
        target: "temper_daemon",
        job_id = %job.job_id,
        repo = %job.repo,
        issue = %number,
        has_source_target_branch = has_non_empty_target_branch(&metadata),
        "forge applier parsed source workflow metadata for verdict children"
    );
    Ok(metadata)
}

fn child_body_with_workflow_metadata(
    body: &str,
    metadata: Option<WorkflowMetadata>,
    kind: &ArtifactKindId,
    branch: ChildBranchBinding<'_>,
) -> Result<String, String> {
    let mut metadata = metadata.unwrap_or_default();
    let mut changed = false;
    if metadata.kind.is_none() {
        metadata.kind = Some(kind.clone());
        changed = true;
    }
    if let Some(enforced_target_branch) = branch.enforced {
        // Authoritative validation already rejected any explicit divergence.
        // Always bind the engine-resolved value so this defense-in-depth layer
        // can never preserve or introduce a child override.
        if metadata.target_branch.as_deref() != Some(enforced_target_branch) {
            metadata.target_branch = Some(enforced_target_branch.to_string());
            changed = true;
        }
    } else if !has_non_empty_target_branch(&metadata) {
        // Legacy transitions without a typed branch policy retain their
        // historical inherit-unless-overridden behavior.
        if let Some(source_target_branch) = branch.legacy_source {
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
