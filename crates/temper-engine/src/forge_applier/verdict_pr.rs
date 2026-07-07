// SPDX-License-Identifier: MPL-2.0

//! Binding metadata-driven pull-request creation for routed issue verdicts.

use temper_forge::{BranchRef, CreatePullRequest, Forge, Issue, ItemNumber, Repository};
use temper_runner::pr_correlation_key;
use temper_workflow::{
    ArtifactKindId, ArtifactRef, ArtifactTarget, Effect, ExecutionContext, TransitionId,
    ValidatedWorkflow, WorkflowMetadata, parse_metadata_block, render_metadata_block,
};

use crate::InFlightJob;
use crate::forge_applier::ForgeApplier;
use crate::workflow_meta::{artifact_kind_create_labels, default_base_branch};

/// Carries the arguments for binding metadata-driven pull-request creation on a
/// routed issue verdict.
pub(super) struct VerdictPullRequestBinding<'a> {
    pub(super) job: &'a InFlightJob,
    pub(super) repository: &'a Repository,
    pub(super) issue: &'a Issue,
    pub(super) artifact_kind: &'a str,
    pub(super) routed: &'a TransitionId,
    pub(super) number: ItemNumber,
    pub(super) title: Option<&'a str>,
    pub(super) body: Option<&'a str>,
    pub(super) context: &'a mut ExecutionContext,
}

impl<F: Forge + ?Sized> ForgeApplier<F> {
    pub(super) fn bind_metadata_pull_request_creates(
        &self,
        binding: VerdictPullRequestBinding<'_>,
    ) -> bool {
        let create_effects =
            create_pull_request_artifact_kind_effects(self.workflow.as_ref(), binding.routed);
        if create_effects.is_empty() {
            return true;
        }

        let Some(source_branch) = issue_metadata_target_branch(binding.job, binding.issue) else {
            return false;
        };
        let target_branch = default_base_branch(binding.repository);
        let authored_text_unambiguous = authored_pr_text_unambiguous(
            self.workflow.as_ref(),
            binding.routed,
            create_effects.len(),
        );
        let authored_title = authored_text_unambiguous.then_some(binding.title).flatten();
        let authored_body = authored_text_unambiguous.then_some(binding.body).flatten();
        let base_correlation_key =
            pr_correlation_key(&ArtifactKindId::new(binding.artifact_kind), binding.number);

        for (ordinal, (effect_index, artifact_kind)) in create_effects.iter().enumerate() {
            let Some(kind) = self.workflow.artifact_kind(artifact_kind) else {
                tracing::warn!(
                    target: "temper_daemon",
                    job_id = %binding.job.job_id,
                    repo = %binding.job.repo,
                    issue = %binding.number,
                    routed = %binding.routed,
                    artifact_kind = %artifact_kind,
                    "forge applier dropped verdict apply with unknown create_pull_request artifact kind"
                );
                return false;
            };
            if kind.target != ArtifactTarget::PullRequest {
                tracing::warn!(
                    target: "temper_daemon",
                    job_id = %binding.job.job_id,
                    repo = %binding.job.repo,
                    issue = %binding.number,
                    routed = %binding.routed,
                    artifact_kind = %artifact_kind,
                    artifact_target = %kind.target,
                    "forge applier dropped verdict apply with non-PR create_pull_request artifact kind"
                );
                return false;
            }

            let metadata = WorkflowMetadata {
                kind: Some(artifact_kind.clone()),
                parents: vec![ArtifactRef::same_repo(binding.number)],
                ..WorkflowMetadata::default()
            };
            let input = CreatePullRequest {
                title: landing_pr_title(authored_title, &source_branch, binding.number),
                body: landing_pr_body(
                    authored_body,
                    &source_branch,
                    &target_branch,
                    binding.number,
                    &metadata,
                ),
                source: BranchRef {
                    repository_id: binding.repository.id.clone(),
                    branch: source_branch.clone(),
                },
                target: BranchRef {
                    repository_id: binding.repository.id.clone(),
                    branch: target_branch.clone(),
                },
                labels: artifact_kind_create_labels(self.workflow.as_ref(), artifact_kind.as_str()),
                assignees: Vec::new(),
            };
            binding.context.set_pull_request_create_at(
                binding.routed.clone(),
                *effect_index,
                input,
            );
            binding.context.set_pull_request_correlation_key_at(
                binding.routed.clone(),
                *effect_index,
                create_pull_request_correlation_key(
                    &base_correlation_key,
                    create_effects.len(),
                    ordinal,
                    artifact_kind,
                ),
            );
        }
        true
    }
}

fn create_pull_request_artifact_kind_effects(
    workflow: &ValidatedWorkflow,
    transition: &TransitionId,
) -> Vec<(usize, ArtifactKindId)> {
    let Some(transition) = workflow
        .transitions()
        .iter()
        .find(|candidate| &candidate.id == transition)
    else {
        return Vec::new();
    };

    let mut create_index = 0;
    let mut effects = Vec::new();
    for effect in &transition.effects {
        if let Effect::CreatePullRequest { artifact_kind, .. } = effect {
            if let Some(artifact_kind) = artifact_kind {
                effects.push((create_index, artifact_kind.clone()));
            }
            create_index += 1;
        }
    }
    effects
}

fn authored_pr_text_unambiguous(
    workflow: &ValidatedWorkflow,
    transition: &TransitionId,
    create_effect_count: usize,
) -> bool {
    if create_effect_count != 1 {
        return false;
    }
    workflow
        .transitions()
        .iter()
        .find(|candidate| &candidate.id == transition)
        .is_some_and(|transition| {
            !transition.effects.iter().any(|effect| {
                matches!(effect, Effect::SetBody { .. } | Effect::AttachReview { .. })
            })
        })
}

fn issue_metadata_target_branch(job: &InFlightJob, issue: &Issue) -> Option<String> {
    let metadata = match parse_metadata_block(&issue.body) {
        Ok(metadata) => metadata,
        Err(error) => {
            tracing::warn!(
                target: "temper_daemon",
                job_id = %job.job_id,
                repo = %job.repo,
                issue = %issue.number,
                %error,
                "forge applier dropped verdict PR create with malformed issue workflow metadata"
            );
            return None;
        }
    };
    let Some(branch) = metadata.and_then(|metadata| metadata.target_branch) else {
        tracing::warn!(
            target: "temper_daemon",
            job_id = %job.job_id,
            repo = %job.repo,
            issue = %issue.number,
            "forge applier dropped verdict PR create without issue target_branch metadata"
        );
        return None;
    };
    let branch = branch.trim();
    if branch.is_empty() {
        tracing::warn!(
            target: "temper_daemon",
            job_id = %job.job_id,
            repo = %job.repo,
            issue = %issue.number,
            "forge applier dropped verdict PR create with blank issue target_branch metadata"
        );
        return None;
    }
    Some(branch.to_string())
}

fn create_pull_request_correlation_key(
    base: &str,
    create_effect_count: usize,
    ordinal: usize,
    artifact_kind: &ArtifactKindId,
) -> String {
    if create_effect_count == 1 {
        base.to_string()
    } else {
        format!("{base}-{}-{}", artifact_kind.as_str(), ordinal)
    }
}

fn landing_pr_title(
    authored_title: Option<&str>,
    source_branch: &str,
    source_issue: ItemNumber,
) -> String {
    authored_title
        .and_then(non_blank)
        .map(str::to_string)
        .unwrap_or_else(|| {
            format!(
                "Land feature branch {source_branch} for issue #{}",
                source_issue.get()
            )
        })
}

fn landing_pr_body(
    authored_body: Option<&str>,
    source_branch: &str,
    target_branch: &str,
    source_issue: ItemNumber,
    metadata: &WorkflowMetadata,
) -> String {
    match authored_body.and_then(non_blank) {
        Some(body) => format!("{}\n\n{}", body, render_metadata_block(metadata)),
        None => format!(
            "Aggregate landing PR from feature branch `{source_branch}` to `{target_branch}` for source issue #{}.\n\n{}",
            source_issue.get(),
            render_metadata_block(metadata)
        ),
    }
}

fn non_blank(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}
