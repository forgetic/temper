use temper_forge::{IssueQuery, IssueState, ItemListDetails, PullRequestQuery, PullRequestState};
use temper_workflow::{
    ArtifactTarget, CompiledWorkflow, Effect, GateCondition, LabelId, QueueManifest, RoleId,
    ValidatedTransition, ValidatedWorkflow,
};

/// Breadth of artifact listing a scan should plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanMode {
    /// Normal role/work scans. When a role is supplied, only that role's
    /// subscribed queues contribute candidate queries.
    Normal,
    /// Normal mechanical automation scans. Only queues that declare automation
    /// metadata contribute candidate queries, and no audit/recovery interest is
    /// added.
    Automated,
    /// Broader audit scans. Audit includes all queues for candidate discovery
    /// and adds workflow-label recovery interest while still avoiding unlabelled
    /// closed history; callers may still filter emitted work to one role.
    Audit,
}

/// Forge list queries needed to fetch scan candidates.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CandidateQueryPlan {
    /// Issue queries, each conjunctive by state and labels.
    pub issue_queries: Vec<IssueQuery>,
    /// Pull-request queries, each conjunctive by state and labels.
    pub pull_request_queries: Vec<PullRequestQuery>,
}

/// Plans Forge list queries from queue interest.
///
/// Open candidates are queried by open state. Closed issue and closed/merged PR
/// candidates are queried only with non-empty queue or audit/recovery labels, so
/// normal scans never request all closed history.
pub fn candidate_query_plan(
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    role: Option<&RoleId>,
    mode: ScanMode,
) -> CandidateQueryPlan {
    let queues = queues_for_scan(compiled, role, mode);
    let mut builder = CandidateQueryBuilder::default();

    for queue in queues {
        let targets = queue_targets(workflow, queue);
        let label_sets = queue_label_sets(queue);
        for target in targets {
            builder.add_queue_interest(target, &label_sets);
        }
    }

    if mode == ScanMode::Audit {
        let targets = workflow_targets(workflow);
        for target in targets {
            let recovery_label_sets = terminal_workflow_label_sets(workflow, target);
            builder.add_recovery_interest(target, &recovery_label_sets);
        }
    }

    builder.build()
}

pub(crate) fn queues_for_scan<'a>(
    compiled: &'a CompiledWorkflow,
    role: Option<&RoleId>,
    mode: ScanMode,
) -> Vec<&'a QueueManifest> {
    compiled
        .queues()
        .iter()
        .filter(|queue| match (mode, role) {
            (ScanMode::Automated, _) => queue.automation.is_some(),
            (ScanMode::Audit, _) | (ScanMode::Normal, None) => true,
            (ScanMode::Normal, Some(role)) => compiled
                .role(role)
                .is_some_and(|manifest| manifest.queues.contains(&queue.id)),
        })
        .collect()
}

#[derive(Default)]
struct CandidateQueryBuilder {
    issue_open_all: bool,
    pull_request_open_all: bool,
    issue_open_labels: Vec<Vec<String>>,
    issue_closed_labels: Vec<Vec<String>>,
    pull_request_open_labels: Vec<Vec<String>>,
    pull_request_closed_labels: Vec<Vec<String>>,
}

impl CandidateQueryBuilder {
    fn add_queue_interest(&mut self, target: ArtifactTarget, label_sets: &[Vec<String>]) {
        self.add_open_interest(target, label_sets);
        self.add_closed_interest(target, label_sets);
    }

    fn add_recovery_interest(&mut self, target: ArtifactTarget, label_sets: &[Vec<String>]) {
        for labels in label_sets.iter().filter(|labels| !labels.is_empty()) {
            self.push_open_labels(target, labels.clone());
            self.push_closed_labels(target, labels.clone());
        }
    }

    fn add_open_interest(&mut self, target: ArtifactTarget, label_sets: &[Vec<String>]) {
        for labels in label_sets {
            if labels.is_empty() {
                match target {
                    ArtifactTarget::Issue => self.issue_open_all = true,
                    ArtifactTarget::PullRequest => self.pull_request_open_all = true,
                }
            } else {
                self.push_open_labels(target, labels.clone());
            }
        }
    }

    fn add_closed_interest(&mut self, target: ArtifactTarget, label_sets: &[Vec<String>]) {
        for labels in label_sets.iter().filter(|labels| !labels.is_empty()) {
            self.push_closed_labels(target, labels.clone());
        }
    }

    fn push_open_labels(&mut self, target: ArtifactTarget, labels: Vec<String>) {
        match target {
            ArtifactTarget::Issue => push_unique_label_set(&mut self.issue_open_labels, labels),
            ArtifactTarget::PullRequest => {
                push_unique_label_set(&mut self.pull_request_open_labels, labels);
            }
        }
    }

    fn push_closed_labels(&mut self, target: ArtifactTarget, labels: Vec<String>) {
        match target {
            ArtifactTarget::Issue => push_unique_label_set(&mut self.issue_closed_labels, labels),
            ArtifactTarget::PullRequest => {
                push_unique_label_set(&mut self.pull_request_closed_labels, labels);
            }
        }
    }

    fn build(self) -> CandidateQueryPlan {
        let mut issue_queries = Vec::new();
        if self.issue_open_all {
            issue_queries.push(issue_query(IssueState::Open, Vec::new()));
        } else {
            issue_queries.extend(
                self.issue_open_labels
                    .into_iter()
                    .map(|labels| issue_query(IssueState::Open, labels)),
            );
        }
        issue_queries.extend(
            self.issue_closed_labels
                .into_iter()
                .map(|labels| issue_query(IssueState::Closed, labels)),
        );

        let mut pull_request_queries = Vec::new();
        if self.pull_request_open_all {
            pull_request_queries.push(pull_request_query(PullRequestState::Open, Vec::new()));
        } else {
            pull_request_queries.extend(
                self.pull_request_open_labels
                    .into_iter()
                    .map(|labels| pull_request_query(PullRequestState::Open, labels)),
            );
        }
        for labels in self.pull_request_closed_labels {
            pull_request_queries.push(pull_request_query(PullRequestState::Closed, labels.clone()));
            pull_request_queries.push(pull_request_query(PullRequestState::Merged, labels));
        }

        CandidateQueryPlan {
            issue_queries,
            pull_request_queries,
        }
    }
}

fn queue_targets(workflow: &ValidatedWorkflow, queue: &QueueManifest) -> Vec<ArtifactTarget> {
    let mut targets = Vec::new();
    for artifact in &queue.artifacts {
        let Some(kind) = workflow.artifact_kind(artifact) else {
            continue;
        };
        if !targets.contains(&kind.target) {
            targets.push(kind.target);
        }
    }
    targets
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

fn queue_label_sets(queue: &QueueManifest) -> Vec<Vec<String>> {
    let base = normalize_labels(queue.labels.iter());
    if queue.any_of.is_empty() {
        return vec![base];
    }

    queue
        .any_of
        .iter()
        .map(|branch| {
            let mut labels = base.clone();
            for label in &branch.labels {
                push_label(&mut labels, label);
            }
            labels
        })
        .collect()
}

fn terminal_workflow_label_sets(
    workflow: &ValidatedWorkflow,
    target: ArtifactTarget,
) -> Vec<Vec<String>> {
    let mut interest = Vec::new();

    record_state_labels(workflow, target, &mut interest);
    record_queue_labels(workflow, target, &mut interest);
    record_transition_effect_labels(workflow, target, &mut interest);
    record_gate_condition_labels(workflow, target, &mut interest);

    workflow
        .labels()
        .iter()
        .filter(|label| interest.contains(label))
        .map(|label| vec![label.as_str().to_string()])
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
                push_label_id(labels, label);
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
            push_label_id(labels, label);
        }
        for label_set in &queue.any_of {
            for label in &label_set.labels {
                push_label_id(labels, label);
            }
        }
        if let Some(condition) = &queue.condition {
            if let Some(label) = condition_label(workflow, condition) {
                push_label_id(labels, label);
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
                push_label_id(labels, label);
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
                push_label_id(labels, label);
            }
        }
    }
}

fn state_allows_target(
    workflow: &ValidatedWorkflow,
    state: &temper_workflow::ValidatedState,
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
        | Effect::SetBody { .. }
        | Effect::AttachReview { .. }
        | Effect::MergePullRequest => None,
    }
}

fn push_label_id(labels: &mut Vec<LabelId>, label: &LabelId) {
    if !labels.contains(label) {
        labels.push(label.clone());
    }
}

fn normalize_labels<'a>(labels: impl IntoIterator<Item = &'a LabelId>) -> Vec<String> {
    let mut normalized = Vec::new();
    for label in labels {
        push_label(&mut normalized, label);
    }
    normalized
}

fn push_label(labels: &mut Vec<String>, label: &LabelId) {
    let label = label.as_str().to_string();
    if !labels.contains(&label) {
        labels.push(label);
    }
}

fn push_unique_label_set(label_sets: &mut Vec<Vec<String>>, labels: Vec<String>) {
    if !label_sets.contains(&labels) {
        label_sets.push(labels);
    }
}

fn issue_query(state: IssueState, labels: Vec<String>) -> IssueQuery {
    IssueQuery {
        state: Some(state),
        labels,
        details: ItemListDetails::summary(),
        ..IssueQuery::default()
    }
}

fn pull_request_query(state: PullRequestState, labels: Vec<String>) -> PullRequestQuery {
    PullRequestQuery {
        state: Some(state),
        labels,
        details: ItemListDetails::summary(),
        ..PullRequestQuery::default()
    }
}
