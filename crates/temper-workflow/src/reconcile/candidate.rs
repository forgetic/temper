use super::{ArtifactSnapshot, ReconcileError, Reconciler, RecoveryPolicy};
use crate::classify::{ArtifactSource, Classifier};
use crate::ids::{ArtifactKindId, LabelId};
use crate::validated::{Effect, GateCondition, ValidatedTransition, ValidatedWorkflow};
use crate::ArtifactTarget;
use temper_forge::{
    Forge, Issue, IssueQuery, IssueState, ItemListDetails, PullRequest, PullRequestQuery,
    PullRequestState, RepositoryId,
};

/// Forge list queries used to discover bounded reconciliation candidates.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReconciliationCandidateQueryPlan {
    /// Issue queries, each conjunctive by explicit state and one workflow label.
    pub issue_queries: Vec<IssueQuery>,
    /// Pull-request queries, each conjunctive by explicit state and one workflow label.
    pub pull_request_queries: Vec<PullRequestQuery>,
}

/// Plans workflow-labelled candidate queries from a validated workflow alone.
///
/// Every declared workflow label is treated as OR interest for open artifacts.
/// Terminal artifacts use only labels that can indicate active or recoverable
/// workflow work for that Forge target. Each emitted query uses one label, an
/// explicit state, and summary list detail, so bounded reconciliation never asks
/// for unlabelled closed or merged history, nor for terminal artifacts carrying
/// only pure artifact-kind identity labels.
pub fn reconciliation_candidate_query_plan(
    workflow: &ValidatedWorkflow,
) -> ReconciliationCandidateQueryPlan {
    let targets = workflow_targets(workflow);
    let labels = workflow_labels(workflow);
    let mut plan = ReconciliationCandidateQueryPlan::default();

    if targets.contains(&ArtifactTarget::Issue) {
        for label in &labels {
            plan.issue_queries
                .push(issue_query(IssueState::Open, label.clone()));
        }
        for label in terminal_workflow_labels(workflow, ArtifactTarget::Issue) {
            plan.issue_queries
                .push(issue_query(IssueState::Closed, label));
        }
    }

    if targets.contains(&ArtifactTarget::PullRequest) {
        for label in &labels {
            plan.pull_request_queries
                .push(pull_request_query(PullRequestState::Open, label.clone()));
        }
        for label in terminal_workflow_labels(workflow, ArtifactTarget::PullRequest) {
            plan.pull_request_queries
                .push(pull_request_query(PullRequestState::Closed, label.clone()));
            plan.pull_request_queries
                .push(pull_request_query(PullRequestState::Merged, label));
        }
    }

    plan
}

impl<'a, P: RecoveryPolicy> Reconciler<'a, P> {
    /// Returns the workflow-labelled query plan used by bounded reconciliation.
    pub fn candidate_query_plan(&self) -> ReconciliationCandidateQueryPlan {
        reconciliation_candidate_query_plan(self.workflow)
    }

    /// Loads workflow-labelled reconciliation candidates with summary list
    /// queries and exact dependency detail only for dependency-gated kinds.
    pub async fn load_bounded_candidate_snapshots<F>(
        &self,
        forge: &F,
        repo_id: &RepositoryId,
    ) -> Result<Vec<ArtifactSnapshot>, ReconcileError>
    where
        F: Forge + ?Sized,
    {
        let plan = self.candidate_query_plan();
        let classifier = Classifier::new(self.workflow);
        let mut snapshots = Vec::new();
        let mut seen = Vec::new();

        for query in plan.issue_queries {
            for issue in forge.list_issues(repo_id, query).await? {
                let source = ArtifactSource::Issue {
                    number: issue.number,
                };
                if seen.contains(&source) {
                    continue;
                }
                seen.push(source);
                if let Some(snapshot) = self
                    .snapshot_for_issue_candidate(forge, repo_id, &classifier, issue)
                    .await?
                {
                    snapshots.push(snapshot);
                }
            }
        }

        for query in plan.pull_request_queries {
            for pull_request in forge.list_pull_requests(repo_id, query).await? {
                let source = ArtifactSource::PullRequest {
                    number: pull_request.number,
                };
                if seen.contains(&source) {
                    continue;
                }
                seen.push(source);
                if let Some(snapshot) = self
                    .snapshot_for_pull_request_candidate(forge, repo_id, &classifier, pull_request)
                    .await?
                {
                    snapshots.push(snapshot);
                }
            }
        }

        Ok(snapshots)
    }

    async fn snapshot_for_issue_candidate<F>(
        &self,
        forge: &F,
        repo_id: &RepositoryId,
        classifier: &Classifier<'_>,
        issue: Issue,
    ) -> Result<Option<ArtifactSnapshot>, ReconcileError>
    where
        F: Forge + ?Sized,
    {
        if self.issue_candidate_needs_dependency_detail(classifier, &issue) {
            return Ok(forge
                .get_issue_by_number(repo_id, issue.number)
                .await?
                .as_ref()
                .map(ArtifactSnapshot::from_issue));
        }
        Ok(Some(ArtifactSnapshot::from_issue(&issue)))
    }

    async fn snapshot_for_pull_request_candidate<F>(
        &self,
        forge: &F,
        repo_id: &RepositoryId,
        classifier: &Classifier<'_>,
        pull_request: PullRequest,
    ) -> Result<Option<ArtifactSnapshot>, ReconcileError>
    where
        F: Forge + ?Sized,
    {
        if self.pull_request_candidate_needs_dependency_detail(classifier, &pull_request) {
            return Ok(forge
                .get_pull_request_by_number(repo_id, pull_request.number)
                .await?
                .as_ref()
                .map(ArtifactSnapshot::from_pull_request));
        }
        Ok(Some(ArtifactSnapshot::from_pull_request(&pull_request)))
    }

    fn issue_candidate_needs_dependency_detail(
        &self,
        classifier: &Classifier<'_>,
        issue: &Issue,
    ) -> bool {
        classifier
            .classify_issue(issue)
            .ok()
            .is_some_and(|artifact| self.kind_has_dependency_gated_recovery(&artifact.kind))
    }

    fn pull_request_candidate_needs_dependency_detail(
        &self,
        classifier: &Classifier<'_>,
        pull_request: &PullRequest,
    ) -> bool {
        classifier
            .classify_pull_request(pull_request)
            .ok()
            .is_some_and(|artifact| self.kind_has_dependency_gated_recovery(&artifact.kind))
    }

    fn kind_has_dependency_gated_recovery(&self, kind: &ArtifactKindId) -> bool {
        self.workflow.transitions().iter().any(|transition| {
            &transition.artifact == kind && requires_dependency_gate(self.workflow, transition)
        })
    }
}

fn workflow_targets(workflow: &ValidatedWorkflow) -> Vec<ArtifactTarget> {
    let mut targets = Vec::new();
    for kind in workflow.artifact_kinds() {
        if !targets.contains(&kind.target) {
            targets.push(kind.target);
        }
    }
    targets
}

fn workflow_labels(workflow: &ValidatedWorkflow) -> Vec<String> {
    let mut labels = Vec::new();
    for label in workflow.labels() {
        let label = label.as_str().to_string();
        if !labels.contains(&label) {
            labels.push(label);
        }
    }
    labels
}

fn terminal_workflow_labels(workflow: &ValidatedWorkflow, target: ArtifactTarget) -> Vec<String> {
    let mut interest = Vec::new();

    record_state_labels(workflow, target, &mut interest);
    record_queue_labels(workflow, target, &mut interest);
    record_transition_effect_labels(workflow, target, &mut interest);
    record_gate_condition_labels(workflow, target, &mut interest);

    workflow
        .labels()
        .iter()
        .filter(|label| interest.contains(label))
        .map(|label| label.as_str().to_string())
        .collect()
}

fn record_state_labels(
    workflow: &ValidatedWorkflow,
    target: ArtifactTarget,
    labels: &mut Vec<LabelId>,
) {
    for dimension in workflow.state_dimensions() {
        for state in &dimension.states {
            if !state_allows_target(workflow, state, target) {
                continue;
            }
            if let Some(label) = &state.label {
                push_label(labels, label);
            }
        }
    }
}

fn record_queue_labels(
    workflow: &ValidatedWorkflow,
    target: ArtifactTarget,
    labels: &mut Vec<LabelId>,
) {
    for queue in workflow.queues() {
        if !queue.artifacts.iter().any(|artifact| {
            workflow
                .artifact_kind(artifact)
                .is_some_and(|kind| kind.target == target)
        }) {
            continue;
        }
        for label in &queue.labels {
            push_label(labels, label);
        }
        for label_set in &queue.any_of {
            for label in &label_set.labels {
                push_label(labels, label);
            }
        }
        if let Some(condition) = &queue.condition {
            if let Some(label) = condition_label(workflow, condition) {
                push_label(labels, label);
            }
        }
    }
}

fn record_transition_effect_labels(
    workflow: &ValidatedWorkflow,
    target: ArtifactTarget,
    labels: &mut Vec<LabelId>,
) {
    for transition in workflow.transitions() {
        if !transition_targets(workflow, transition, target) {
            continue;
        }
        for effect in &transition.effects {
            if let Some(label) = effect_label(effect) {
                push_label(labels, label);
            }
        }
    }
}

fn record_gate_condition_labels(
    workflow: &ValidatedWorkflow,
    target: ArtifactTarget,
    labels: &mut Vec<LabelId>,
) {
    for gate in workflow.gates() {
        if !workflow.transitions().iter().any(|transition| {
            transition.requires_gates.contains(&gate.id)
                && transition_targets(workflow, transition, target)
        }) {
            continue;
        }
        if let Some(condition) = &gate.condition {
            if let Some(label) = condition_label(workflow, condition) {
                push_label(labels, label);
            }
        }
    }
}

fn state_allows_target(
    workflow: &ValidatedWorkflow,
    state: &crate::validated::ValidatedState,
    target: ArtifactTarget,
) -> bool {
    state.artifacts.is_empty()
        || state.artifacts.iter().any(|artifact| {
            workflow
                .artifact_kind(artifact)
                .is_some_and(|kind| kind.target == target)
        })
}

fn transition_targets(
    workflow: &ValidatedWorkflow,
    transition: &ValidatedTransition,
    target: ArtifactTarget,
) -> bool {
    workflow
        .artifact_kind(&transition.artifact)
        .is_some_and(|kind| kind.target == target)
}

fn condition_label<'a>(
    workflow: &'a ValidatedWorkflow,
    condition: &'a GateCondition,
) -> Option<&'a LabelId> {
    match condition {
        GateCondition::LabelPresent(label) => Some(label),
        GateCondition::StateEquals { dimension, state } => workflow
            .state_dimensions()
            .iter()
            .find(|candidate| &candidate.id == dimension)?
            .states
            .iter()
            .find(|candidate| &candidate.id == state)?
            .label
            .as_ref(),
        GateCondition::DependenciesResolved
        | GateCondition::CiPassed
        | GateCondition::CiFailed
        | GateCondition::ReviewApproved
        | GateCondition::ReviewChangesRequested => None,
    }
}

fn effect_label(effect: &Effect) -> Option<&LabelId> {
    match effect {
        Effect::AddLabel(label) | Effect::RemoveLabel(label) => Some(label),
        Effect::SetAssignee(_)
        | Effect::RemoveAssignee(_)
        | Effect::CreateComment { .. }
        | Effect::CreatePullRequest { .. }
        | Effect::RequestReviewers { .. }
        | Effect::SubmitReview { .. }
        | Effect::MergePullRequest => None,
    }
}

fn push_label(labels: &mut Vec<LabelId>, label: &LabelId) {
    if !labels.contains(label) {
        labels.push(label.clone());
    }
}

fn requires_dependency_gate(
    workflow: &ValidatedWorkflow,
    transition: &ValidatedTransition,
) -> bool {
    transition.requires_gates.iter().any(|gate_id| {
        workflow.gates().iter().any(|gate| {
            &gate.id == gate_id
                && matches!(gate.condition, Some(GateCondition::DependenciesResolved))
        })
    })
}

fn issue_query(state: IssueState, label: String) -> IssueQuery {
    IssueQuery {
        state: Some(state),
        labels: vec![label],
        details: ItemListDetails::summary(),
        ..IssueQuery::default()
    }
}

fn pull_request_query(state: PullRequestState, label: String) -> PullRequestQuery {
    PullRequestQuery {
        state: Some(state),
        labels: vec![label],
        details: ItemListDetails::summary(),
        ..PullRequestQuery::default()
    }
}
