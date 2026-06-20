// SPDX-License-Identifier: MPL-2.0

//! Success-path application: opening one coordinated implementation PR per
//! writable repo that produced a diff, in coordinated landing order (ADR 0023).

use std::collections::BTreeMap;

use temper_forge::{
    Forge, ItemNumber, PullRequest, Repository, RepositoryId, RepositoryPath, RequestReviewers,
    UpdatePullRequest, UserId,
};
use temper_log::emit::{PrOpened, emit_pr_opened};
use temper_protocol_worker::{JobContext, JobResult, RepoOutcome};
use temper_workflow::{ArtifactKindId, ArtifactSource, Effect, Executor};

use temper_runner::{artifact_ref, pr_correlation_key};

use crate::InFlightJob;
use crate::forge_applier::ForgeApplier;
use crate::forge_applier::coordinated::{
    CoordinatedSet, coordinated_landing_order, coordinated_pr_pull_request_input,
    manifest_depends_on,
};
use crate::forge_applier::run_ledger::RunLedgerPullRequest;
use crate::workflow_meta::{
    default_base_branch, implementation_pr_create_labels, implementation_pr_labels,
};

impl<F: Forge + ?Sized> ForgeApplier<F> {
    pub(super) async fn apply_success(&self, job: InFlightJob, result: JobResult) {
        if result.verdict.is_some() {
            self.apply_verdict(job, result).await;
            return;
        }

        if result.repos.is_empty() {
            tracing::warn!(
                target: "temper_daemon",
                job_id = %job.job_id,
                repo = %job.repo,
                artifact_kind = %job.artifact.kind,
                artifact_item = %job.artifact.item,
                "forge applier ignored success result with no repo outcomes"
            );
            return;
        }

        // A pull-request job (e.g. a CI-failure fix) pushes its diff straight to
        // the existing PR head branch; that push itself re-triggers CI and the
        // landing queue takes over once CI is green. There is no new PR to open,
        // so the success-path PR-opening below (which keys on a coordinating
        // *issue*) does not apply.
        if job.artifact.kind == "pull_request" {
            // §5: a between-cause detail (the push that re-triggers CI), not a
            // §7 catalog state change — belongs at debug.
            tracing::debug!(
                target: "temper_daemon",
                job_id = %job.job_id,
                repo = %job.repo,
                artifact_item = %job.artifact.item,
                head_branch = %result
                    .repos
                    .first()
                    .map(|repo| repo.branch.name.as_str())
                    .unwrap_or("-"),
                "forge applier pushed pull-request fix to head; awaiting fresh CI"
            );
            return;
        }

        // The coordinating issue lives in the primary repo; every PR in the set
        // links back to it with a repo-qualified ref (ADR 0023).
        let Some((primary_repository, issue)) = self.resolve_issue(&job).await else {
            return;
        };
        let number = issue.number;

        let context = match serde_json::from_value::<JobContext>(job.job_payload.clone()) {
            Ok(context) => context,
            Err(error) => {
                tracing::error!(
                    target: "temper_daemon",
                    job_id = %job.job_id,
                    repo = %job.repo,
                    issue = %number,
                    %error,
                    "forge applier could not parse JobContext"
                );
                return;
            }
        };
        self.apply_source_action_claim(&job).await;

        let source_kind = ArtifactKindId::new(context.artifact_kind.clone());
        // The coordination key keys every PR in the set; fall back to the
        // single-issue correlation key when the payload carries no manifest.
        let coordination_key = context
            .workspace
            .as_ref()
            .map(|workspace| workspace.coordination_key.clone())
            .unwrap_or_else(|| pr_correlation_key(&source_kind, number));

        let lookup_labels = implementation_pr_labels(self.workflow.as_ref());
        let create_labels = implementation_pr_create_labels(self.workflow.as_ref());
        let worker_id = result.worker_id.clone();
        let summary = result.summary.unwrap_or_default();

        // Open one PR per writable repo that produced a diff, in coordinated
        // landing order: a repo's PR is created after the PRs it depends on, so
        // their numbers are known and can be wired as cross-repo dependency
        // links (ADR 0023, acyclic). The `dependency_gate` then holds each PR
        // closed until its prerequisites merge.
        let depends_on = manifest_depends_on(&context);
        let order = coordinated_landing_order(&result.repos, &depends_on);
        let mut opened: BTreeMap<String, (RepositoryId, ItemNumber)> = BTreeMap::new();

        let set = CoordinatedSet {
            job: &job,
            primary_id: &primary_repository.id,
            issue_title: &issue.title,
            number,
            summary: &summary,
            coordination_key: &coordination_key,
            lookup_labels: &lookup_labels,
            create_labels: &create_labels,
            depends_on: &depends_on,
        };
        for index in order {
            self.open_coordinated_pr(&set, &result.repos[index], &mut opened)
                .await;
        }
        if !opened.is_empty() {
            self.clear_source_action_working_labels(&job).await;
            let pull_requests = opened
                .iter()
                .map(|(repo, (_, number))| RunLedgerPullRequest {
                    repo: repo.clone(),
                    number: *number,
                })
                .collect::<Vec<_>>();
            self.finalize_run_ledger(&job, &coordination_key, &worker_id, &pull_requests)
                .await;
        }
    }

    /// Opens (or ensures) the coordinated PR for one repo outcome, recording the
    /// opened PR in `opened` so later dependents can wire dependency links.
    pub(super) async fn open_coordinated_pr(
        &self,
        set: &CoordinatedSet<'_>,
        outcome: &RepoOutcome,
        opened: &mut BTreeMap<String, (RepositoryId, ItemNumber)>,
    ) {
        if outcome.branch.name.trim().is_empty() {
            tracing::warn!(
                target: "temper_daemon",
                job_id = %set.job.job_id,
                repo = %set.job.repo,
                outcome_repo = %outcome.repo,
                "forge applier ignored blank branch in repo outcome"
            );
            return;
        }
        let Some(target_repository) = self.resolve_repo_path(set.job, &outcome.repo).await else {
            return;
        };
        let base_branch = default_base_branch(&target_repository);
        let coordinating = if &target_repository.id == set.primary_id {
            temper_workflow::ArtifactRef::same_repo(set.number)
        } else {
            temper_workflow::ArtifactRef::in_repo(set.primary_id.clone(), set.number)
        };
        let dependencies = set.dependency_refs(&outcome.repo, opened);

        let input = coordinated_pr_pull_request_input(
            target_repository.id.clone(),
            coordinating,
            set.number,
            set.issue_title,
            outcome.branch.name.clone(),
            base_branch,
            set.summary,
            set.create_labels.to_vec(),
            set.coordination_key,
            dependencies,
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
                            "final success",
                        )
                        .await;
                }
                self.apply_implementation_pr_handoff_if_needed(set.job, &pull_request, was_created)
                    .await;
                opened.insert(
                    outcome.repo.clone(),
                    (target_repository.id.clone(), pull_request.number),
                );
            }
            Err(error) => tracing::error!(
                target: "temper_daemon",
                job_id = %set.job.job_id,
                repo = %set.job.repo,
                issue = %set.number,
                target_repo = %outcome.repo,
                coordination_key = %set.coordination_key,
                %error,
                "forge applier ensure_pull_request failed"
            ),
        }
    }

    /// Resolves an `owner/name` repo path (a [`RepoOutcome::repo`]) to its Forge
    /// [`Repository`]. Logs and returns `None` on a malformed path or lookup
    /// miss.
    pub(super) async fn resolve_repo_path(
        &self,
        job: &InFlightJob,
        path: &str,
    ) -> Option<Repository> {
        let Some((owner, name)) = path.split_once('/') else {
            tracing::warn!(
                target: "temper_daemon",
                job_id = %job.job_id,
                outcome_repo = %path,
                "forge applier ignored malformed repo outcome path"
            );
            return None;
        };
        match self
            .forge
            .get_repository_by_path(&RepositoryPath::new(owner, name))
            .await
        {
            Ok(Some(repository)) => Some(repository),
            Ok(None) => {
                tracing::warn!(
                    target: "temper_daemon",
                    job_id = %job.job_id,
                    outcome_repo = %path,
                    "forge applier repo outcome repository not found"
                );
                None
            }
            Err(error) => {
                tracing::error!(
                    target: "temper_daemon",
                    job_id = %job.job_id,
                    outcome_repo = %path,
                    %error,
                    "forge applier repo outcome lookup failed"
                );
                None
            }
        }
    }

    async fn apply_implementation_pr_handoff_if_needed(
        &self,
        job: &InFlightJob,
        pull_request: &PullRequest,
        was_created: bool,
    ) {
        let handoff = self.implementation_pr_review_handoff(&job.role);
        if handoff.is_empty() {
            return;
        }

        let has_working_label = handoff
            .remove_labels
            .iter()
            .any(|label| pull_request.labels.contains(label));
        let has_review_label = handoff
            .add_labels
            .iter()
            .any(|label| pull_request.labels.contains(label));
        if !(was_created || has_working_label || has_review_label) {
            return;
        }

        if was_created || has_working_label {
            let update = UpdatePullRequest {
                add_labels: handoff.add_labels,
                remove_labels: handoff.remove_labels,
                ..UpdatePullRequest::default()
            };
            if let Err(error) = self
                .forge
                .update_pull_request(&pull_request.id, update)
                .await
            {
                tracing::warn!(
                    target: "temper_daemon",
                    job_id = %job.job_id,
                    pull_request = %pull_request.number,
                    %error,
                    "forge applier could not apply implementation PR review labels"
                );
            }
        }

        let reviewers = handoff
            .reviewers
            .into_iter()
            .filter(|reviewer| !pull_request.requested_reviewers.contains(reviewer))
            .collect::<Vec<_>>();
        if reviewers.is_empty() {
            return;
        }
        if let Err(error) = self
            .forge
            .request_pull_request_reviewers(&pull_request.id, RequestReviewers { reviewers })
            .await
        {
            tracing::warn!(
                target: "temper_daemon",
                job_id = %job.job_id,
                pull_request = %pull_request.number,
                %error,
                "forge applier could not request implementation PR review"
            );
        }
    }

    fn implementation_pr_review_handoff(&self, role: &str) -> ReviewHandoff {
        let implementation_pr = ArtifactKindId::new("implementation_pr");
        let preferred = self.workflow.transitions().iter().find(|transition| {
            transition.id.as_str() == "request_review"
                && transition.artifact == implementation_pr
                && transition
                    .roles
                    .iter()
                    .any(|candidate| candidate.as_str() == role)
        });
        let fallback = || {
            self.workflow.transitions().iter().find(|transition| {
                transition.artifact == implementation_pr
                    && transition
                        .roles
                        .iter()
                        .any(|candidate| candidate.as_str() == role)
                    && transition
                        .effects
                        .iter()
                        .any(|effect| matches!(effect, Effect::RequestReviewers { .. }))
            })
        };
        let Some(transition) = preferred.or_else(fallback) else {
            return ReviewHandoff::default();
        };

        let mut handoff = ReviewHandoff::default();
        for effect in &transition.effects {
            match effect {
                Effect::AddLabel(label) => push_unique(&mut handoff.add_labels, label.as_str()),
                Effect::RemoveLabel(label) | Effect::RemoveLabelIfPresent(label) => {
                    push_unique(&mut handoff.remove_labels, label.as_str());
                }
                Effect::RequestReviewers { roles } => {
                    for role in roles {
                        let reviewer = UserId::new(role.as_str());
                        if !handoff.reviewers.contains(&reviewer) {
                            handoff.reviewers.push(reviewer);
                        }
                    }
                }
                _ => {}
            }
        }
        handoff
    }
}

#[derive(Default)]
struct ReviewHandoff {
    add_labels: Vec<String>,
    remove_labels: Vec<String>,
    reviewers: Vec<UserId>,
}

impl ReviewHandoff {
    fn is_empty(&self) -> bool {
        self.add_labels.is_empty() && self.remove_labels.is_empty() && self.reviewers.is_empty()
    }
}

fn push_unique(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|candidate| candidate == value) {
        values.push(value.to_string());
    }
}
