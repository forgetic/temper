use temper_forge::{
    CandidateLabelSelection, CandidateLifecycle, IssueCandidateQuery, ItemListDetails,
    PullRequestCandidateQuery,
};
use temper_workflow::{
    ArtifactTarget, CompiledWorkflow, QueueManifest, RoleId, ValidatedWorkflow, workflow_interest,
};

/// Breadth of artifact listing a scan should plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanMode {
    /// Normal role/work scans, scoped to the selected role when supplied.
    Normal,
    /// Wake-triggered role/work scans, with the same bounded recovery interest.
    Wake,
    /// Open-only queues carrying automation metadata.
    Automated,
    /// All queues plus bounded terminal recovery interest.
    Audit,
}

/// Consolidated Forge candidate buckets needed for one scan.
///
/// Each vector contains at most one open and one terminal query. Terminal
/// queries are always label-bounded; an unfiltered open query dominates other
/// open label interest for the same artifact target.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CandidateQueryPlan {
    pub issue_queries: Vec<IssueCandidateQuery>,
    pub pull_request_queries: Vec<PullRequestCandidateQuery>,
}

/// Plans one role, audit, wake, or automated candidate pass.
pub fn candidate_query_plan(
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    role: Option<&RoleId>,
    mode: ScanMode,
) -> CandidateQueryPlan {
    candidate_query_plan_for_queues(workflow, queues_for_scan(compiled, role, mode), mode)
}

/// Plans one candidate pass for the union of queues subscribed by `roles`.
pub fn candidate_query_plan_for_roles(
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    roles: &[RoleId],
    mode: ScanMode,
) -> CandidateQueryPlan {
    candidate_query_plan_for_queues(workflow, queues_for_roles(compiled, roles), mode)
}

fn candidate_query_plan_for_queues(
    workflow: &ValidatedWorkflow,
    queues: Vec<&QueueManifest>,
    mode: ScanMode,
) -> CandidateQueryPlan {
    let mut builder = CandidateQueryBuilder::default();
    for queue in queues {
        for target in queue_targets(workflow, queue) {
            builder.add_open_queue(target, queue);
        }
    }

    if !matches!(mode, ScanMode::Automated) {
        let interest = workflow_interest(workflow);
        for &target in interest.targets() {
            builder.add_terminal(target, interest.terminal_labels(target));
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
            (ScanMode::Audit, _) | (ScanMode::Normal, None) | (ScanMode::Wake, None) => true,
            (ScanMode::Normal | ScanMode::Wake, Some(role)) => compiled
                .role(role)
                .is_some_and(|manifest| manifest.queues.contains(&queue.id)),
        })
        .collect()
}

pub(crate) fn queues_for_roles<'a>(
    compiled: &'a CompiledWorkflow,
    roles: &[RoleId],
) -> Vec<&'a QueueManifest> {
    compiled
        .queues()
        .iter()
        .filter(|queue| {
            roles.iter().any(|role| {
                compiled
                    .role(role)
                    .is_some_and(|manifest| manifest.queues.contains(&queue.id))
            })
        })
        .collect()
}

#[derive(Default)]
struct CandidateQueryBuilder {
    issue_open_all: bool,
    pull_request_open_all: bool,
    issue_open_labels: Vec<String>,
    issue_terminal_labels: Vec<String>,
    pull_request_open_labels: Vec<String>,
    pull_request_terminal_labels: Vec<String>,
}

impl CandidateQueryBuilder {
    fn add_open_queue(&mut self, target: ArtifactTarget, queue: &QueueManifest) {
        let labels = queue_discovery_labels(queue);
        if labels.is_empty() {
            match target {
                ArtifactTarget::Issue => self.issue_open_all = true,
                ArtifactTarget::PullRequest => self.pull_request_open_all = true,
            }
            return;
        }
        let destination = match target {
            ArtifactTarget::Issue => &mut self.issue_open_labels,
            ArtifactTarget::PullRequest => &mut self.pull_request_open_labels,
        };
        extend_unique(destination, labels);
    }

    fn add_terminal(&mut self, target: ArtifactTarget, labels: &[String]) {
        if labels.is_empty() {
            return;
        }
        let destination = match target {
            ArtifactTarget::Issue => &mut self.issue_terminal_labels,
            ArtifactTarget::PullRequest => &mut self.pull_request_terminal_labels,
        };
        extend_unique(destination, labels.iter().cloned());
    }

    fn build(mut self) -> CandidateQueryPlan {
        normalize(&mut self.issue_open_labels);
        normalize(&mut self.issue_terminal_labels);
        normalize(&mut self.pull_request_open_labels);
        normalize(&mut self.pull_request_terminal_labels);

        let mut plan = CandidateQueryPlan::default();
        if self.issue_open_all {
            plan.issue_queries.push(issue_candidate(
                CandidateLifecycle::Open,
                CandidateLabelSelection::Unfiltered,
            ));
        } else if !self.issue_open_labels.is_empty() {
            plan.issue_queries.push(issue_candidate(
                CandidateLifecycle::Open,
                CandidateLabelSelection::AnyOf(self.issue_open_labels),
            ));
        }
        if !self.issue_terminal_labels.is_empty() {
            plan.issue_queries.push(issue_candidate(
                CandidateLifecycle::Terminal,
                CandidateLabelSelection::AnyOf(self.issue_terminal_labels),
            ));
        }

        if self.pull_request_open_all {
            plan.pull_request_queries.push(pull_request_candidate(
                CandidateLifecycle::Open,
                CandidateLabelSelection::Unfiltered,
            ));
        } else if !self.pull_request_open_labels.is_empty() {
            plan.pull_request_queries.push(pull_request_candidate(
                CandidateLifecycle::Open,
                CandidateLabelSelection::AnyOf(self.pull_request_open_labels),
            ));
        }
        if !self.pull_request_terminal_labels.is_empty() {
            plan.pull_request_queries.push(pull_request_candidate(
                CandidateLifecycle::Terminal,
                CandidateLabelSelection::AnyOf(self.pull_request_terminal_labels),
            ));
        }
        plan
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

/// Positive queue labels are only a discovery superset. Their conjunction and
/// any-of branch structure remain local classifier/queue matching rules.
fn queue_discovery_labels(queue: &QueueManifest) -> Vec<String> {
    // With no common labels, an empty disjunct is itself an unfiltered match
    // and must dominate labelled sibling branches just like a label-free queue.
    if queue.labels.is_empty()
        && (queue.any_of.is_empty() || queue.any_of.iter().any(|branch| branch.labels.is_empty()))
    {
        return Vec::new();
    }

    let mut labels = Vec::new();
    extend_unique(
        &mut labels,
        queue.labels.iter().map(|label| label.as_str().to_string()),
    );
    for branch in &queue.any_of {
        extend_unique(
            &mut labels,
            branch.labels.iter().map(|label| label.as_str().to_string()),
        );
    }
    labels
}

fn extend_unique(labels: &mut Vec<String>, values: impl IntoIterator<Item = String>) {
    for label in values {
        if !labels.contains(&label) {
            labels.push(label);
        }
    }
}

fn normalize(labels: &mut Vec<String>) {
    labels.sort();
    labels.dedup();
}

fn issue_candidate(
    lifecycle: CandidateLifecycle,
    labels: CandidateLabelSelection,
) -> IssueCandidateQuery {
    IssueCandidateQuery {
        lifecycle,
        labels,
        details: ItemListDetails::summary(),
    }
}

fn pull_request_candidate(
    lifecycle: CandidateLifecycle,
    labels: CandidateLabelSelection,
) -> PullRequestCandidateQuery {
    PullRequestCandidateQuery {
        lifecycle,
        labels,
        details: ItemListDetails::summary(),
    }
}
