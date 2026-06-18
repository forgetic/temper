// SPDX-License-Identifier: MPL-2.0

//! Plan-publication progress application for plan-first implementation PRs.

use std::collections::{BTreeMap, BTreeSet};

use temper_forge::{Forge, ItemNumber, RepositoryId};
use temper_log::emit::{PrOpened, emit_pr_opened};
use temper_protocol_worker::{
    Branch, JobContext, JobPlanPublication, JobPlanPublicationTarget, JobProgress, RepoAccess,
    RepoOutcome,
};
use temper_runner::{artifact_ref, pr_correlation_key};
use temper_workflow::{ArtifactKindId, ArtifactSource, Executor};

use crate::InFlightJob;
use crate::forge_applier::ForgeApplier;
use crate::forge_applier::coordinated::{
    CoordinatedSet, coordinated_landing_order, coordinated_pr_pull_request_input,
    manifest_depends_on,
};
use crate::workflow_meta::{
    default_base_branch, implementation_pr_labels, implementation_pr_plan_create_labels,
};

impl<F: Forge + ?Sized> ForgeApplier<F> {
    pub(super) async fn apply_plan_publication_progress(
        &self,
        job: &InFlightJob,
        progress: &JobProgress,
    ) -> bool {
        let Some(publication) = progress.plan_publication.as_ref() else {
            return false;
        };

        let context = match serde_json::from_value::<JobContext>(job.job_payload.clone()) {
            Ok(context) => context,
            Err(error) => {
                tracing::warn!(
                    target: "temper_daemon",
                    job_id = %job.job_id,
                    %error,
                    "forge applier could not parse JobContext for plan publication"
                );
                return true;
            }
        };
        if !is_writable_engineer_issue_job(job, &context) {
            return true;
        }

        let Some((primary_repository, issue)) = self.resolve_issue(job).await else {
            return true;
        };
        let number = issue.number;
        let source_kind = ArtifactKindId::new(context.artifact_kind.clone());
        let coordination_key = context
            .workspace
            .as_ref()
            .map(|workspace| workspace.coordination_key.clone())
            .unwrap_or_else(|| pr_correlation_key(&source_kind, number));
        if progress.correlation_key != coordination_key {
            tracing::warn!(
                target: "temper_daemon",
                job_id = %job.job_id,
                progress_key = %progress.correlation_key,
                coordination_key = %coordination_key,
                "forge applier plan publication correlation key differed from JobContext"
            );
        }

        let targets = plan_targets(&context, publication);
        if targets.is_empty() {
            tracing::warn!(
                target: "temper_daemon",
                job_id = %job.job_id,
                correlation_key = %coordination_key,
                "forge applier ignored plan publication with no writable targets"
            );
            return true;
        }

        let lookup_labels = implementation_pr_labels(self.workflow.as_ref());
        let create_labels = implementation_pr_plan_create_labels(self.workflow.as_ref());
        let depends_on = manifest_depends_on(&context);
        let outcomes = targets
            .iter()
            .map(PlanTarget::as_ordering_outcome)
            .collect::<Vec<_>>();
        let order = coordinated_landing_order(&outcomes, &depends_on);
        let mut opened: BTreeMap<String, (RepositoryId, ItemNumber)> = BTreeMap::new();

        let set = CoordinatedSet {
            job,
            primary_id: &primary_repository.id,
            issue_title: &issue.title,
            number,
            summary: &publication.summary,
            coordination_key: &coordination_key,
            lookup_labels: &lookup_labels,
            create_labels: &create_labels,
            plan_phases: &publication.phases,
            depends_on: &depends_on,
        };
        for index in order {
            self.ensure_plan_pr(&set, &targets[index], &mut opened)
                .await;
        }
        true
    }

    async fn ensure_plan_pr(
        &self,
        set: &CoordinatedSet<'_>,
        target: &PlanTarget,
        opened: &mut BTreeMap<String, (RepositoryId, ItemNumber)>,
    ) {
        let Some(target_repository) = self.resolve_repo_path(set.job, &target.repo).await else {
            return;
        };
        let base_branch = if target.base_branch.trim().is_empty() {
            default_base_branch(&target_repository)
        } else {
            target.base_branch.clone()
        };
        let coordinating = if &target_repository.id == set.primary_id {
            temper_workflow::ArtifactRef::same_repo(set.number)
        } else {
            temper_workflow::ArtifactRef::in_repo(set.primary_id.clone(), set.number)
        };
        let dependencies = set.dependency_refs(&target.repo, opened);
        let input = coordinated_pr_pull_request_input(
            target_repository.id.clone(),
            coordinating,
            set.number,
            set.issue_title,
            target.branch.clone(),
            base_branch,
            set.summary,
            set.create_labels.to_vec(),
            set.coordination_key,
            dependencies,
            set.plan_phases,
        );
        let desired_body = input.body.clone();

        match Executor::new(self.workflow.as_ref(), self.forge.as_ref())
            .ensure_pull_request_with_lookup(
                &target_repository.id,
                set.coordination_key,
                set.lookup_labels,
                input,
            )
            .await
        {
            Ok(ensured) => {
                let was_created = ensured.was_created();
                let mut pull_request = ensured.into_artifact();
                if was_created {
                    let pr_ref = artifact_ref(
                        &target_repository.id,
                        ArtifactSource::PullRequest {
                            number: pull_request.number,
                        },
                    );
                    emit_pr_opened(PrOpened {
                        item: &pr_ref,
                        title: set.issue_title,
                        kind: "implementation",
                        for_issue: set.number.get(),
                    });
                } else {
                    pull_request = self
                        .update_implementation_pr_body(
                            set.job,
                            pull_request,
                            &desired_body,
                            "plan publication",
                        )
                        .await;
                }
                opened.insert(
                    target.repo.clone(),
                    (target_repository.id.clone(), pull_request.number),
                );
            }
            Err(error) => tracing::error!(
                target: "temper_daemon",
                job_id = %set.job.job_id,
                repo = %set.job.repo,
                issue = %set.number,
                target_repo = %target.repo,
                coordination_key = %set.coordination_key,
                %error,
                "forge applier ensure plan-first pull request failed"
            ),
        }
    }
}

#[derive(Clone, Debug)]
struct PlanTarget {
    repo: String,
    branch: String,
    base_branch: String,
}

impl PlanTarget {
    fn as_ordering_outcome(&self) -> RepoOutcome {
        RepoOutcome {
            repo: self.repo.clone(),
            branch: Branch {
                name: self.branch.clone(),
                head_sha: String::new(),
            },
        }
    }
}

fn is_writable_engineer_issue_job(job: &InFlightJob, context: &JobContext) -> bool {
    if job.role != "engineer" || job.artifact.kind != "issue" {
        return false;
    }
    if context.checkout_capability.as_deref() == Some("writable") {
        return true;
    }
    context
        .workspace
        .as_ref()
        .is_some_and(|workspace| workspace.writable().next().is_some())
}

fn plan_targets(context: &JobContext, publication: &JobPlanPublication) -> Vec<PlanTarget> {
    let writable = context
        .workspace
        .as_ref()
        .map(|workspace| {
            workspace
                .repos
                .iter()
                .filter(|repo| repo.access == RepoAccess::Writable)
                .map(|repo| repo.repo.as_str())
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();

    let mut targets = Vec::new();
    for target in publication.target_repos.iter().filter_map(|target| {
        target
            .branch_hint
            .as_deref()
            .map(str::trim)
            .filter(|branch| !branch.is_empty())
            .map(|branch| (target, branch))
    }) {
        if !writable.is_empty() && !writable.contains(target.0.repo_path.as_str()) {
            continue;
        }
        push_target(&mut targets, target.0, target.1);
    }

    if targets.is_empty()
        && let Some(workspace) = context.workspace.as_ref()
    {
        for repo in workspace.writable() {
            if let Some(branch) = repo.branch_hint.as_deref().map(str::trim)
                && !branch.is_empty()
            {
                targets.push(PlanTarget {
                    repo: repo.repo.clone(),
                    branch: branch.to_string(),
                    base_branch: repo.base_branch.clone(),
                });
            }
        }
    }
    targets
}

fn push_target(
    targets: &mut Vec<PlanTarget>,
    publication: &JobPlanPublicationTarget,
    branch: &str,
) {
    if targets
        .iter()
        .any(|existing| existing.repo == publication.repo_path)
    {
        return;
    }
    targets.push(PlanTarget {
        repo: publication.repo_path.clone(),
        branch: branch.to_string(),
        base_branch: publication.base_branch.clone(),
    });
}
