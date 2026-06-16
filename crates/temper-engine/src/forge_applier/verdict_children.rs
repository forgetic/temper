// SPDX-License-Identifier: MPL-2.0

//! Binding a verdict's `create_issues` children into the execution context:
//! mapping each [`JobChild`] to a [`CreateIssuesChild`] (resolving cross-repo
//! targets) and stamping the deterministic content correlation key.

use temper_forge::{Forge, ItemNumber, Repository, RepositoryId};
use temper_worker_protocol::JobChild;
use temper_workflow::{ArtifactKindId, CreateIssuesChild};

use temper_runner::workspace_content_key;

use crate::InFlightJob;
use crate::forge_applier::ForgeApplier;
use crate::forge_applier::verdict::{
    VerdictChildrenBinding, create_issues_effect_index, parse_child_target_repo,
};
use crate::workflow_meta::code_child_create_labels;

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

        let mut mapped = Vec::with_capacity(binding.children.len());
        for child in binding.children {
            let Some(mapped_child) = self
                .map_job_child(binding.job, binding.repository_id, binding.number, child)
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
        child: JobChild,
    ) -> Option<CreateIssuesChild> {
        // Breakdown children are `code` work items. The labels that route a code
        // issue to the engineer's queue are declared by the workflow (the `code`
        // artifact-kind's identifying + initial labels), NOT left to the agent: a
        // child created label-less (or missing the activation label) would be
        // classified as the catch-all `intake` kind and spuriously re-triaged,
        // wiping its parent back-reference. Union the workflow-required labels with
        // whatever the agent authored so the child is always created
        // engineer-ready, exactly as the single-repo triage path is.
        let mut labels = code_child_create_labels(self.workflow.as_ref());
        for label in child.labels {
            if !labels.contains(&label) {
                labels.push(label);
            }
        }
        let mut mapped = CreateIssuesChild {
            slug: child.slug,
            title: child.title,
            body: child.body,
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
