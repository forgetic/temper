//! Effect application for the [`Executor`].
//!
//! This child module holds the mutation half of the runtime loop: turning a
//! plan's [`WorkflowEffect`]s into Forge calls. It is split from the parent
//! `execute` module to keep both files within the source-size budget; it accesses
//! the parent's private [`Executor`] and [`Loaded`] items as a descendant
//! module. Postcondition verification and the reconciler's label-only apply path
//! live in [`verify`](super::verify); the comment/review and issue-fan-out apply
//! paths live in [`messaging`](super::messaging) and
//! [`issue_creates`](super::issue_creates).
//!
//! The application discipline is "validate everything, then mutate": unsupported
//! effects, missing create inputs, and unbound assignee roles fail before any
//! backend call. Idempotent comments and pull-request creates run before the
//! merge (if any) and final label/assignee update — the commit point. See the
//! parent module docs for why pre-commit effects are ordered this way.

use super::issue_creates::{
    PreparedCreateIssues, create_issues_completion, validate_child_dependencies,
};
use super::messaging::PreparedAttachReview;
use super::verify::AppliedState;
use super::{ExecutionError, Executor, Loaded};
use crate::context::CreateIssuesChild;
use crate::ids::{ArtifactKindId, RoleId};
use crate::plan::{TransitionPlan, WorkflowEffect};
use temper_forge::{
    CreatePullRequest, Forge, ForgeError, IssueState, RepositoryId, ReviewDecision, UpdateIssue,
    UpdatePullRequest, UserId,
};

impl<F: Forge + ?Sized> Executor<'_, F> {
    /// Applies a plan's effects, refusing partial application of the state flip.
    ///
    /// Validates effects before mutating, runs pre-commit idempotent effects,
    /// then folds labels and assignees into one backend update. Postconditions
    /// are checked against that update result, not a later reload that another
    /// worker may already have advanced.
    pub(super) async fn apply(
        &self,
        repo_id: &RepositoryId,
        loaded: &Loaded,
        plan: &TransitionPlan,
    ) -> Result<(), ExecutionError> {
        let prepared = self.prepare_effects(plan)?;
        validate_pull_request_effects(loaded, &prepared)?;
        self.apply_comments(loaded, &plan.transition, &prepared.comments)
            .await?;
        self.apply_pull_request_creates(repo_id, &prepared.pull_request_creates)
            .await?;
        let create_completion = create_issues_completion(
            prepared.body.as_deref(),
            &prepared.add_labels,
            &prepared.remove_labels,
            &prepared.add_assignees,
            &prepared.remove_assignees,
        );
        let create_committed = self
            .apply_issue_creates(
                repo_id,
                plan.target,
                &prepared.issue_creates,
                &create_completion,
            )
            .await?;
        self.apply_review_requests(loaded, &prepared.review_requests)
            .await?;
        self.apply_reviews(loaded, &plan.transition, &prepared.reviews)
            .await?;
        self.apply_attach_reviews(loaded, &prepared.attach_reviews)
            .await?;
        self.apply_merge(
            repo_id,
            loaded,
            prepared.merge,
            prepared.delete_source_branch_on_merge,
        )
        .await?;
        self.apply_close_parent_issues(repo_id, loaded, prepared.close_parent_issues)
            .await?;
        let committed = if create_committed.is_some() {
            // Fan-out completion atomically folded the durable intent progress
            // into the routed source update, so applying `prepared` again would
            // split the transition's commit point.
            create_committed
        } else {
            self.apply_update(repo_id, loaded, Some(plan), prepared)
                .await?
        };
        if let Some(state) = committed {
            self.verify_state(&state, &plan.postconditions)?;
        } else {
            self.verify_current(repo_id, loaded.classified().source, &plan.postconditions)
                .await?;
        }
        Ok(())
    }

    /// Partitions plan effects into concrete backend operations.
    ///
    /// Runs before any mutation so an unsupported effect, missing create input,
    /// missing correlation key, or unbound assignee role fails the whole
    /// transition cleanly, never half-applied.
    fn prepare_effects(&self, plan: &TransitionPlan) -> Result<PreparedEffects, ExecutionError> {
        let mut prepared = PreparedEffects::default();
        let mut counters = EffectCounters::default();
        for effect in &plan.effects {
            self.prepare_effect(plan, effect, &mut prepared, &mut counters)?;
        }
        Ok(prepared)
    }

    /// Partitions a single plan effect into its concrete backend operation.
    fn prepare_effect(
        &self,
        plan: &TransitionPlan,
        effect: &WorkflowEffect,
        prepared: &mut PreparedEffects,
        counters: &mut EffectCounters,
    ) -> Result<(), ExecutionError> {
        match effect {
            WorkflowEffect::AddLabel(label) => {
                prepared.add_labels.push(label.as_str().to_string());
            }
            WorkflowEffect::RemoveLabel(label) => {
                prepared.remove_labels.push(label.as_str().to_string());
            }
            WorkflowEffect::SetAssignee { role } => {
                prepared.add_assignees.push(self.resolve_assignee(role)?);
            }
            WorkflowEffect::RemoveAssignee { role } => {
                prepared.remove_assignees.push(self.resolve_assignee(role)?);
            }
            WorkflowEffect::CreateComment { body } => {
                prepared.comments.push(body.clone());
            }
            WorkflowEffect::CreatePullRequest {
                correlation_key,
                artifact_kind,
            } => {
                let effect_index = counters.pull_request_create;
                counters.pull_request_create += 1;
                let correlation_key = correlation_key
                    .clone()
                    .or_else(|| {
                        self.context
                            .pull_request_correlation_key(&plan.transition, effect_index)
                            .map(str::to_string)
                    })
                    .ok_or_else(|| ExecutionError::MissingCorrelationKey {
                        effect: effect.clone(),
                    })?;
                let input = self
                    .context
                    .pull_request_create(&plan.transition, effect_index)
                    .cloned()
                    .ok_or_else(|| ExecutionError::UnresolvedPullRequestCreate {
                        transition: plan.transition.clone(),
                        effect_index,
                    })?;
                prepared
                    .pull_request_creates
                    .push(PreparedPullRequestCreate {
                        correlation_key,
                        input,
                        artifact_kind: artifact_kind.clone(),
                    });
            }
            WorkflowEffect::RequestReviewers { roles } => {
                for role in roles {
                    let reviewer = self
                        .context
                        .resolve_assignee(role)
                        .cloned()
                        .ok_or_else(|| ExecutionError::UnresolvedReviewer { role: role.clone() })?;
                    prepared.review_requests.push(reviewer);
                }
            }
            WorkflowEffect::SubmitReview { decision } => {
                prepared.reviews.push(*decision);
            }
            WorkflowEffect::SetBody { correlation_key: _ } => {
                let effect_index = counters.set_body;
                counters.set_body += 1;
                // The authored body is the runtime work product; an effect
                // with no bound body fails before any mutation, exactly like
                // a `CreatePullRequest` with no bound create input. The
                // correlation key is accepted for symmetry but `set_body`
                // overwrites and so is naturally idempotent across retries.
                let body = self
                    .context
                    .set_body(&plan.transition, effect_index)
                    .map(str::to_string)
                    .ok_or_else(|| ExecutionError::UnresolvedSetBody {
                        transition: plan.transition.clone(),
                        effect_index,
                    })?;
                prepared.body = Some(body);
            }
            WorkflowEffect::AttachReview {
                decision,
                correlation_key,
            } => {
                let effect_index = counters.attach_review;
                counters.attach_review += 1;
                let correlation_key = correlation_key
                    .clone()
                    .or_else(|| {
                        self.context
                            .attach_review_correlation_key(&plan.transition, effect_index)
                            .map(str::to_string)
                    })
                    .ok_or_else(|| ExecutionError::MissingCorrelationKey {
                        effect: effect.clone(),
                    })?;
                let body = self
                    .context
                    .attach_review(&plan.transition, effect_index)
                    .map(str::to_string)
                    .ok_or_else(|| ExecutionError::UnresolvedAttachReview {
                        transition: plan.transition.clone(),
                        effect_index,
                    })?;
                prepared.attach_reviews.push(PreparedAttachReview {
                    decision: *decision,
                    correlation_key,
                    body,
                });
            }
            WorkflowEffect::CreateIssues {
                correlation_key,
                record_parent_dependencies,
            } => {
                let effect_index = counters.create_issues;
                counters.create_issues += 1;
                // A create is not naturally idempotent, so — like
                // `CreatePullRequest` — a base correlation key is required:
                // each child derives a stable per-child key from it so a
                // retry resolves the existing children instead of
                // duplicating them.
                let base_correlation_key = correlation_key
                    .clone()
                    .or_else(|| {
                        self.context
                            .create_issues_correlation_key(&plan.transition, effect_index)
                            .map(str::to_string)
                    })
                    .ok_or_else(|| ExecutionError::MissingCorrelationKey {
                        effect: effect.clone(),
                    })?;
                let children = self
                    .context
                    .create_issues(&plan.transition, effect_index)
                    .map(<[CreateIssuesChild]>::to_vec)
                    .ok_or_else(|| ExecutionError::UnresolvedCreateIssues {
                        transition: plan.transition.clone(),
                        effect_index,
                    })?;
                validate_child_dependencies(&plan.transition, effect_index, &children)?;
                prepared.issue_creates.push(PreparedCreateIssues {
                    transition: plan.transition.clone(),
                    effect_index,
                    base_correlation_key,
                    children,
                    record_parent_dependencies: *record_parent_dependencies,
                });
            }
            WorkflowEffect::MergePullRequest => {
                prepared.merge = true;
                prepared.delete_source_branch_on_merge = self.is_direct_automation_transition(plan);
            }
            WorkflowEffect::CloseParentIssues => {
                prepared.close_parent_issues = true;
            }
            other => {
                return Err(ExecutionError::UnsupportedEffect {
                    effect: other.clone(),
                });
            }
        }
        Ok(())
    }

    /// Returns whether the plan is the workflow-declared direct mechanical
    /// automation transition. Direct automation merges clean up Temper-owned
    /// source branches after the merge; workspace-backed automation leaves the
    /// workspace's explicit merge request in control.
    fn is_direct_automation_transition(&self, plan: &TransitionPlan) -> bool {
        self.workflow.queues().iter().any(|queue| {
            queue.automation.as_ref().is_some_and(|automation| {
                automation.executor.is_none()
                    && automation.actor == plan.role
                    && automation.transition == plan.transition
            })
        })
    }

    /// Resolves an assignee role to a Forge user through the execution context.
    pub(super) fn resolve_assignee(&self, role: &RoleId) -> Result<UserId, ExecutionError> {
        self.context
            .resolve_assignee(role)
            .cloned()
            .ok_or_else(|| ExecutionError::UnresolvedAssignee { role: role.clone() })
    }

    /// Creates requested pull requests idempotently before the label commit
    /// point.
    ///
    /// If a create lands but a later effect crashes before the source artifact's
    /// label flip, retrying the transition reuses the same correlation key and
    /// resolves to the existing pull request instead of creating a duplicate.
    async fn apply_pull_request_creates(
        &self,
        repo_id: &RepositoryId,
        creates: &[PreparedPullRequestCreate],
    ) -> Result<(), ExecutionError> {
        for create in creates {
            if let Some(artifact_kind) = &create.artifact_kind {
                let lookup_labels = pull_request_lookup_labels(self.workflow, artifact_kind)
                    .unwrap_or_else(|| create.input.labels.clone());
                self.ensure_pull_request_with_lookup(
                    repo_id,
                    &create.correlation_key,
                    &lookup_labels,
                    create.input.clone(),
                )
                .await?;
            } else {
                self.ensure_pull_request(repo_id, &create.correlation_key, create.input.clone())
                    .await?;
            }
        }
        Ok(())
    }

    /// Folds the prepared labels and assignees into a single backend update.
    ///
    /// Skips the call entirely when there is nothing to change (for example a
    /// comment-only transition), so it never issues an empty mutation. When it
    /// does update, returns the backend's committed artifact state for
    /// postcondition checks.
    pub(super) async fn apply_update(
        &self,
        repo_id: &RepositoryId,
        loaded: &Loaded,
        plan: Option<&TransitionPlan>,
        prepared: PreparedEffects,
    ) -> Result<Option<AppliedState>, ExecutionError> {
        if !prepared.has_update() {
            return Ok(None);
        }
        match loaded {
            Loaded::Issue { id, version, .. } => {
                let (id, version) = if let Some(plan) = plan {
                    self.refresh_issue_commit_state(repo_id, plan).await?
                } else {
                    (id.clone(), *version)
                };
                let update = UpdateIssue {
                    body: prepared.body,
                    add_labels: prepared.add_labels,
                    remove_labels: prepared.remove_labels,
                    add_assignees: prepared.add_assignees,
                    remove_assignees: prepared.remove_assignees,
                    expected_version: Some(version),
                    ..UpdateIssue::default()
                };
                let issue = self
                    .forge
                    .update_issue(&id, update)
                    .await
                    .map_err(|error| stale_update_error(error, loaded.classified().source))?;
                Ok(Some(AppliedState {
                    labels: issue.labels,
                    assignees: issue.assignees,
                }))
            }
            Loaded::PullRequest { id, .. } => {
                let update = UpdatePullRequest {
                    body: prepared.body,
                    add_labels: prepared.add_labels,
                    remove_labels: prepared.remove_labels,
                    add_assignees: prepared.add_assignees,
                    remove_assignees: prepared.remove_assignees,
                    ..UpdatePullRequest::default()
                };
                let pull_request = self.forge.update_pull_request(id, update).await?;
                Ok(Some(AppliedState {
                    labels: pull_request.labels,
                    assignees: pull_request.assignees,
                }))
            }
        }
    }

    async fn refresh_issue_commit_state(
        &self,
        repo_id: &RepositoryId,
        plan: &TransitionPlan,
    ) -> Result<(temper_forge::IssueId, temper_forge::Version), ExecutionError> {
        let current = self.load(repo_id, plan.target).await?;
        let needs = self.workflow.signal_needs_for_transition(&plan.transition);
        let signals = self
            .gate_signals_with_needs(repo_id, &current, needs)
            .await?;
        self.workflow
            .planner()
            .plan_transition_with(&plan.transition, &plan.role, current.classified(), &signals)
            .map_err(|error| ExecutionError::TargetStale {
                target: plan.target,
                message: format!("target changed before commit: {error}"),
            })?;
        match current {
            Loaded::Issue { id, version, .. } => Ok((id, version)),
            Loaded::PullRequest { .. } => Err(ExecutionError::TargetStale {
                target: plan.target,
                message: "target changed artifact type before commit".to_string(),
            }),
        }
    }

    /// Closes parent issues of a pull request after merge, using the parent
    /// list parsed during the pre-merge load/classification.
    ///
    /// For each same-repo parent that is still open, closes the issue and
    /// removes the `in-progress` label. Already-closed parents are idempotent
    /// no-ops; missing metadata and missing parent issues are not errors.
    async fn apply_close_parent_issues(
        &self,
        repo_id: &RepositoryId,
        loaded: &Loaded,
        close: bool,
    ) -> Result<(), ExecutionError> {
        if !close {
            return Ok(());
        }
        let Loaded::PullRequest { classified, .. } = loaded else {
            return Err(ExecutionError::UnsupportedEffect {
                effect: WorkflowEffect::CloseParentIssues,
            });
        };
        for parent in &classified.metadata.parents {
            // Resolve same-repo shorthand; cross-repo parents are skipped for now
            if !parent.is_in_repository(repo_id) {
                continue;
            }
            let Some(issue) = self
                .forge
                .get_issue_by_number(repo_id, parent.number)
                .await?
            else {
                continue; // Missing parent — not an error
            };
            let is_open = issue.state != IssueState::Closed;
            let has_in_progress = issue.labels.iter().any(|l| l == "in-progress");
            if !is_open && !has_in_progress {
                continue; // Already fully resolved — idempotent no-op
            }
            self.forge
                .update_issue(
                    &issue.id,
                    UpdateIssue {
                        state: is_open.then_some(IssueState::Closed),
                        remove_labels: if has_in_progress {
                            vec!["in-progress".to_string()]
                        } else {
                            Vec::new()
                        },
                        expected_version: Some(issue.version),
                        ..UpdateIssue::default()
                    },
                )
                .await
                .map_err(|error| {
                    stale_update_error(
                        error,
                        crate::classify::ArtifactSource::Issue {
                            number: issue.number,
                        },
                    )
                })?;
        }
        Ok(())
    }
}

/// Per-effect-kind indices used while partitioning a plan's effects.
///
/// Each content-bearing effect resolves its runtime input by a `(transition,
/// index)` key, so the index counts that effect kind's occurrences in plan
/// order.
#[derive(Default)]
struct EffectCounters {
    pull_request_create: usize,
    set_body: usize,
    attach_review: usize,
    create_issues: usize,
}

/// Effects partitioned into a single backend update plus pre-commit effects.
///
/// Labels and assignees are applied together in one `UpdateIssue`/
/// `UpdatePullRequest` so the state flip is atomic; comments and pull-request
/// creates are applied separately (and idempotently) before that update.
#[derive(Default)]
pub(super) struct PreparedEffects {
    add_labels: Vec<String>,
    remove_labels: Vec<String>,
    add_assignees: Vec<UserId>,
    remove_assignees: Vec<UserId>,
    comments: Vec<String>,
    pull_request_creates: Vec<PreparedPullRequestCreate>,
    issue_creates: Vec<PreparedCreateIssues>,
    review_requests: Vec<UserId>,
    reviews: Vec<ReviewDecision>,
    attach_reviews: Vec<PreparedAttachReview>,
    /// Agent-authored body to write onto the target, folded into the same
    /// atomic label/assignee commit update. The last `SetBody` effect wins.
    body: Option<String>,
    /// Whether the plan requests merging the target pull request.
    merge: bool,
    /// Whether a merge request should delete the PR source branch after a
    /// successful backend merge.
    delete_source_branch_on_merge: bool,
    /// Whether the plan requests closing parent issues of the target PR.
    close_parent_issues: bool,
}

impl PreparedEffects {
    /// Records a label to add in the single commit update.
    pub(super) fn push_add_label(&mut self, label: String) {
        self.add_labels.push(label);
    }

    /// Records a label to remove in the single commit update.
    pub(super) fn push_remove_label(&mut self, label: String) {
        self.remove_labels.push(label);
    }

    /// Returns `true` when the single label/assignee/body update would change
    /// anything, so a comment-only transition never issues an empty mutation.
    fn has_update(&self) -> bool {
        !(self.add_labels.is_empty()
            && self.remove_labels.is_empty()
            && self.add_assignees.is_empty()
            && self.remove_assignees.is_empty()
            && self.body.is_none())
    }
}

/// A concrete, idempotent pull-request create request prepared from a plan
/// effect plus the runtime [`crate::context::ExecutionContext`].
struct PreparedPullRequestCreate {
    correlation_key: String,
    input: CreatePullRequest,
    artifact_kind: Option<ArtifactKindId>,
}

fn pull_request_lookup_labels(
    workflow: &crate::validated::ValidatedWorkflow,
    artifact_kind: &ArtifactKindId,
) -> Option<Vec<String>> {
    workflow.artifact_kind(artifact_kind).map(|kind| {
        kind.identifying_labels
            .iter()
            .map(|label| label.as_str().to_string())
            .collect()
    })
}

fn stale_update_error(
    error: ForgeError,
    target: crate::classify::ArtifactSource,
) -> ExecutionError {
    match error {
        ForgeError::Conflict(message) => ExecutionError::TargetStale { target, message },
        other => other.into(),
    }
}

fn validate_pull_request_effects(
    loaded: &Loaded,
    prepared: &PreparedEffects,
) -> Result<(), ExecutionError> {
    if matches!(loaded, Loaded::PullRequest { .. })
        || (prepared.review_requests.is_empty()
            && prepared.reviews.is_empty()
            && prepared.attach_reviews.is_empty())
    {
        return Ok(());
    }
    let effect = if !prepared.review_requests.is_empty() {
        WorkflowEffect::RequestReviewers { roles: Vec::new() }
    } else if let Some(decision) = prepared.reviews.first().copied() {
        WorkflowEffect::SubmitReview { decision }
    } else {
        WorkflowEffect::AttachReview {
            decision: prepared
                .attach_reviews
                .first()
                .map(|review| review.decision)
                .unwrap_or(ReviewDecision::Commented),
            correlation_key: None,
        }
    };
    Err(ExecutionError::UnsupportedEffect { effect })
}
