// SPDX-License-Identifier: MPL-2.0

//! Success-path application: opening one coordinated implementation PR per
//! writable repo that produced a diff, in coordinated landing order (ADR 0023).

use std::borrow::Cow;
use std::collections::BTreeMap;

use temper_forge::{
    Forge, ItemNumber, PullRequest, Repository, RepositoryId, RepositoryPath, RequestReviewers,
    UpdatePullRequest, UserId,
};
use temper_log::emit::{PrHandoffFacts, PrOpened, PrUpdated, emit_pr_opened, emit_pr_updated};
use temper_protocol_worker::{FailureClass, JobContext, JobResult, RepoOutcome};
use temper_workflow::{
    ArtifactKindId, ArtifactSource, Effect, Executor, TargetBranchPolicy, TransitionId,
    parse_metadata_block, validate_pull_request_topology,
};

use temper_runner::{artifact_ref, pr_correlation_key};

use crate::InFlightJob;
use crate::applier::ApplyOutcome;
use crate::forge_applier::ForgeApplier;
use crate::forge_applier::coordinated::{
    CoordinatedSet, coordinated_landing_order, coordinated_pr_pull_request_input,
    manifest_base_branches, manifest_depends_on, pr_target_branch,
};
use crate::forge_applier::pr_reuse::pull_request_reuse_error;
use crate::workflow_meta::{
    artifact_kind_create_labels, create_pull_request_target_branch_policy,
    success_pull_request_artifact_kind,
};

impl<F: Forge + ?Sized> ForgeApplier<F> {
    pub(super) async fn apply_success(&self, job: InFlightJob, result: JobResult) -> ApplyOutcome {
        if result.verdict.is_some() {
            return self.apply_verdict(job, result).await;
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
            return ApplyOutcome::Rejected {
                class: temper_protocol_worker::FailureClass::Protocol,
                reason: "successful result has neither a verdict nor repository products"
                    .to_string(),
            };
        }

        // A writable pull-request job pushes directly to the existing PR head.
        // Publishing that repair is itself a workflow transition: the repaired
        // head marker and transition labels commit atomically before the
        // human-facing handoff is refreshed.
        if job.artifact.kind == "pull_request" {
            return self.publish_pull_request_repair(&job, &result).await;
        }

        // The coordinating issue lives in the primary repo; every PR in the set
        // links back to it with a repo-qualified ref (ADR 0023).
        let Some((primary_repository, issue)) = self.resolve_issue(&job).await else {
            return ApplyOutcome::Stale;
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
                return ApplyOutcome::Rejected {
                    class: temper_protocol_worker::FailureClass::Protocol,
                    reason: format!("invalid in-flight JobContext: {error}"),
                };
            }
        };
        let base_branches = match self
            .validated_success_base_branches(
                &job,
                &context,
                &primary_repository,
                &issue,
                &result.repos,
            )
            .await
        {
            Ok(Some(base_branches)) => base_branches,
            Ok(None) => manifest_base_branches(&context),
            Err(outcome) => return outcome,
        };

        let source_kind = ArtifactKindId::new(context.artifact_kind.clone());
        // The coordination key keys every PR in the set; fall back to the
        // single-issue correlation key when the payload carries no manifest.
        let coordination_key = context
            .workspace
            .as_ref()
            .map(|workspace| workspace.coordination_key.clone())
            .unwrap_or_else(|| pr_correlation_key(&source_kind, number));

        let pull_request_kind = match context.action.as_deref() {
            Some(action) => match success_pull_request_artifact_kind(
                self.workflow.as_ref(),
                &TransitionId::new(action),
            ) {
                Ok(kind) => kind,
                Err(reason) => return branch_policy_rejected(reason),
            },
            None => ArtifactKindId::new("implementation_pr"),
        };
        let lookup_labels = self
            .workflow
            .artifact_kind(&pull_request_kind)
            .map(|kind| {
                kind.identifying_labels
                    .iter()
                    .map(|label| label.as_str().to_string())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let create_labels =
            artifact_kind_create_labels(self.workflow.as_ref(), pull_request_kind.as_str());
        let summary = result.summary.unwrap_or_default();
        let authored_title = result.title.as_deref();
        let authored_body = result.body.as_deref();

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
            title: authored_title,
            body: authored_body,
            coordination_key: &coordination_key,
            lookup_labels: &lookup_labels,
            create_labels: &create_labels,
            depends_on: &depends_on,
            base_branches: &base_branches,
            pull_request_kind: &pull_request_kind,
        };
        if let Err(outcome) = self
            .validate_existing_coordinated_pr_topologies(&set, &result.repos)
            .await
        {
            return outcome;
        }
        for index in order {
            if let Err(outcome) = self
                .open_coordinated_pr(&set, &result.repos[index], &mut opened)
                .await
            {
                return outcome;
            }
        }
        if !opened.is_empty() {
            self.apply_source_action_claim(&job).await;
            self.clear_source_action_working_labels(&job).await;
        }
        ApplyOutcome::Applied
    }

    /// Re-resolves an explicit PR target policy from fresh source and repository
    /// state immediately before creation. Legacy actions with no policy retain
    /// the historical manifest/default behavior.
    async fn validated_success_base_branches(
        &self,
        job: &InFlightJob,
        context: &JobContext,
        primary_repository: &Repository,
        issue: &temper_forge::Issue,
        outcomes: &[RepoOutcome],
    ) -> Result<Option<BTreeMap<String, String>>, ApplyOutcome> {
        let Some(action) = context.action.as_deref() else {
            return Ok(None);
        };
        let policy = create_pull_request_target_branch_policy(
            self.workflow.as_ref(),
            &TransitionId::new(action),
        )
        .map_err(branch_policy_rejected)?;
        let Some(policy) = policy else {
            // Omitted policy is the compatibility path: old workflows continue
            // to use the assignment manifest and repository-default fallback.
            return Ok(None);
        };
        let workspace = context.workspace.as_ref().ok_or_else(|| {
            branch_policy_rejected(format!(
                "action `{action}` has target-branch policy `{policy}` but no workspace manifest"
            ))
        })?;
        let source_branch = if policy == TargetBranchPolicy::NonDefault {
            let metadata = parse_metadata_block(&issue.body)
                .map_err(|error| {
                    branch_policy_rejected(format!(
                        "fresh source issue workflow metadata is malformed: {error}"
                    ))
                })?
                .ok_or_else(|| {
                    branch_policy_rejected(
                        "fresh source issue has no workflow metadata target branch",
                    )
                })?;
            Some(
                metadata
                    .target_branch
                    .as_deref()
                    .map(str::trim)
                    .filter(|branch| !branch.is_empty())
                    .ok_or_else(|| {
                        branch_policy_rejected(
                            "fresh source issue has no non-blank workflow target branch",
                        )
                    })?
                    .to_string(),
            )
        } else {
            None
        };

        let mut validated = BTreeMap::new();
        for outcome in outcomes {
            if validated.contains_key(&outcome.repo) {
                return Err(branch_policy_rejected(format!(
                    "successful result repeats repository outcome `{}`",
                    outcome.repo
                )));
            }
            let manifest_repos = workspace
                .repos
                .iter()
                .filter(|repo| repo.repo == outcome.repo)
                .collect::<Vec<_>>();
            let [manifest_repo] = manifest_repos.as_slice() else {
                return Err(branch_policy_rejected(format!(
                    "successful result repository `{}` does not have exactly one workspace manifest entry",
                    outcome.repo
                )));
            };
            if !manifest_repo.is_writable() {
                return Err(branch_policy_rejected(format!(
                    "successful result repository `{}` is not writable in the workspace manifest",
                    outcome.repo
                )));
            }

            let target_repository = if outcome.repo == job.repo {
                primary_repository.clone()
            } else {
                let (owner, name) = outcome.repo.split_once('/').ok_or_else(|| {
                    branch_policy_rejected(format!(
                        "successful result has malformed repository path `{}`",
                        outcome.repo
                    ))
                })?;
                if owner.is_empty() || name.is_empty() || name.contains('/') {
                    return Err(branch_policy_rejected(format!(
                        "successful result has malformed repository path `{}`",
                        outcome.repo
                    )));
                }
                self.forge
                    .get_repository_by_path(&RepositoryPath::new(owner, name))
                    .await
                    .map_err(|error| ApplyOutcome::Retryable {
                        reason: format!(
                            "read fresh repository `{}` before pull-request creation: {error}",
                            outcome.repo
                        ),
                    })?
                    .ok_or_else(|| {
                        branch_policy_rejected(format!(
                            "successful result repository `{}` no longer exists",
                            outcome.repo
                        ))
                    })?
            };
            let repository_default = target_repository.default_branch.trim();
            if repository_default.is_empty() {
                return Err(branch_policy_rejected(format!(
                    "fresh repository `{}` has a blank default branch",
                    outcome.repo
                )));
            }
            let expected = match policy {
                TargetBranchPolicy::NonDefault => {
                    let source_branch = source_branch.as_deref().expect("resolved above");
                    if source_branch == repository_default {
                        return Err(branch_policy_rejected(format!(
                            "fresh source target branch `{source_branch}` equals repository `{}` default branch",
                            outcome.repo
                        )));
                    }
                    source_branch
                }
                TargetBranchPolicy::RepositoryDefault => repository_default,
                TargetBranchPolicy::DerivedFeatureBranch | TargetBranchPolicy::Inherit => {
                    return Err(branch_policy_rejected(format!(
                        "action `{action}` has unsupported create_pull_request target-branch policy `{policy}`"
                    )));
                }
            };
            if manifest_repo.base_branch.trim() != expected {
                return Err(branch_policy_rejected(format!(
                    "workspace repository `{}` base `{}` diverges from fresh policy target `{expected}`",
                    outcome.repo, manifest_repo.base_branch
                )));
            }
            validated.insert(outcome.repo.clone(), expected.to_string());
        }
        Ok(Some(validated))
    }

    /// Opens (or ensures) the coordinated PR for one repo outcome, recording the
    /// opened PR in `opened` so later dependents can wire dependency links.
    pub(super) async fn open_coordinated_pr(
        &self,
        set: &CoordinatedSet<'_>,
        outcome: &RepoOutcome,
        opened: &mut BTreeMap<String, (RepositoryId, ItemNumber)>,
    ) -> Result<(), ApplyOutcome> {
        if outcome.branch.name.trim().is_empty() {
            tracing::warn!(
                target: "temper_daemon",
                job_id = %set.job.job_id,
                repo = %set.job.repo,
                outcome_repo = %outcome.repo,
                "forge applier ignored blank branch in repo outcome"
            );
            return Ok(());
        }
        let Some(target_repository) = self.resolve_repo_path(set.job, &outcome.repo).await else {
            return Ok(());
        };
        let base_branch = pr_target_branch(set, &outcome.repo, &target_repository);
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
            set.title,
            set.body,
            set.create_labels.to_vec(),
            set.coordination_key,
            dependencies,
            set.pull_request_kind,
        );
        let desired_title = input.title.clone();
        let desired_body = input.body.clone();
        let desired_source = input.source.clone();
        let desired_target = input.target.clone();

        if let Some(pull_request) = self
            .existing_open_pr_for_branch(
                set.job,
                &target_repository.id,
                &outcome.branch.name,
                set.lookup_labels,
            )
            .await
            .map_err(pull_request_reuse_error)?
        {
            validate_pull_request_topology(&pull_request, &desired_source, &desired_target)
                .map_err(pull_request_reuse_error)?;
            self.update_existing_coordinated_pr(
                set,
                outcome,
                opened,
                &target_repository.id,
                pull_request,
                &desired_title,
                &desired_body,
                "source branch reuse",
            )
            .await;
            return Ok(());
        }

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
                let pull_request = ensured.into_artifact();
                if was_created {
                    self.record_created_coordinated_pr(
                        set,
                        outcome,
                        opened,
                        &target_repository.id,
                        pull_request,
                    )
                    .await;
                } else {
                    // The executor validates correlation candidates before it
                    // returns them. Keep this local check as a final guard at
                    // the handoff boundary.
                    validate_pull_request_topology(&pull_request, &desired_source, &desired_target)
                        .map_err(pull_request_reuse_error)?;
                    self.update_existing_coordinated_pr(
                        set,
                        outcome,
                        opened,
                        &target_repository.id,
                        pull_request,
                        &desired_title,
                        &desired_body,
                        "final success",
                    )
                    .await;
                }
                Ok(())
            }
            Err(error) => {
                tracing::error!(
                    target: "temper_daemon",
                    job_id = %set.job.job_id,
                    repo = %set.job.repo,
                    issue = %set.number,
                    target_repo = %outcome.repo,
                    coordination_key = %set.coordination_key,
                    %error,
                    "forge applier ensure_pull_request failed"
                );
                // A create can land even if the backend response is lost. Keep
                // the historical branch fallback for that case, but subject it
                // to the same immutable topology check as every other reuse.
                if let Some(pull_request) = self
                    .existing_open_pr_for_branch(
                        set.job,
                        &target_repository.id,
                        &outcome.branch.name,
                        set.lookup_labels,
                    )
                    .await
                    .map_err(pull_request_reuse_error)?
                {
                    validate_pull_request_topology(&pull_request, &desired_source, &desired_target)
                        .map_err(pull_request_reuse_error)?;
                    self.update_existing_coordinated_pr(
                        set,
                        outcome,
                        opened,
                        &target_repository.id,
                        pull_request,
                        &desired_title,
                        &desired_body,
                        "ensure fallback",
                    )
                    .await;
                    Ok(())
                } else {
                    Err(pull_request_reuse_error(error))
                }
            }
        }
    }

    async fn record_created_coordinated_pr(
        &self,
        set: &CoordinatedSet<'_>,
        outcome: &RepoOutcome,
        opened: &mut BTreeMap<String, (RepositoryId, ItemNumber)>,
        target_repo_id: &RepositoryId,
        pull_request: PullRequest,
    ) {
        let pr_ref = artifact_ref(
            target_repo_id,
            ArtifactSource::PullRequest {
                number: pull_request.number,
            },
        );
        emit_pr_opened(PrOpened {
            item: &pr_ref,
            title: &pull_request.title,
            kind: set.pull_request_kind.as_str(),
            for_issue: set.number.get(),
            handoff: Some(pr_handoff_facts(
                set,
                &pull_request.title,
                set.body.is_some(),
                "created",
            )),
        });
        self.apply_implementation_pr_handoff_if_needed(set.job, &pull_request, true)
            .await;
        opened.insert(
            outcome.repo.clone(),
            (target_repo_id.clone(), pull_request.number),
        );
    }

    #[allow(clippy::too_many_arguments)]
    async fn update_existing_coordinated_pr(
        &self,
        set: &CoordinatedSet<'_>,
        outcome: &RepoOutcome,
        opened: &mut BTreeMap<String, (RepositoryId, ItemNumber)>,
        target_repo_id: &RepositoryId,
        pull_request: PullRequest,
        desired_title: &str,
        desired_body: &str,
        operation: &'static str,
    ) {
        let result = self
            .update_implementation_pr_handoff(
                set.job,
                pull_request,
                desired_title,
                desired_body,
                operation,
            )
            .await;
        let pull_request = result.pull_request;
        if result.updated {
            let pr_ref = artifact_ref(
                target_repo_id,
                ArtifactSource::PullRequest {
                    number: pull_request.number,
                },
            );
            emit_pr_updated(PrUpdated {
                item: &pr_ref,
                kind: set.pull_request_kind.as_str(),
                for_issue: set.number.get(),
                handoff: pr_handoff_facts(
                    set,
                    &pull_request.title,
                    set.body.is_some(),
                    "refreshed",
                ),
            });
        }
        self.apply_implementation_pr_handoff_if_needed(set.job, &pull_request, false)
            .await;
        opened.insert(
            outcome.repo.clone(),
            (target_repo_id.clone(), pull_request.number),
        );
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

fn branch_policy_rejected(reason: impl Into<String>) -> ApplyOutcome {
    ApplyOutcome::Rejected {
        class: FailureClass::Protocol,
        reason: reason.into(),
    }
}

fn pr_handoff_facts(
    set: &CoordinatedSet<'_>,
    title: &str,
    body_authored: bool,
    action: &'static str,
) -> PrHandoffFacts<'static> {
    let source_ref =
        artifact_ref(set.primary_id, ArtifactSource::Issue { number: set.number }).to_string();
    PrHandoffFacts {
        source_artifact: Cow::Owned(source_ref.clone()),
        title: Cow::Owned(title.to_string()),
        title_source: Cow::Borrowed(if set.title.is_some() {
            "agent"
        } else {
            "fallback"
        }),
        body_source: Cow::Borrowed(if body_authored {
            "agent"
        } else {
            "summary_fallback"
        }),
        metadata_kind: Cow::Owned(set.pull_request_kind.as_str().to_string()),
        metadata_parent_ref: Cow::Owned(source_ref),
        correlation_key: Cow::Owned(set.coordination_key.to_string()),
        action: Cow::Borrowed(action),
    }
}

fn push_unique(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|candidate| candidate == value) {
        values.push(value.to_string());
    }
}
