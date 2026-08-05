//! Shared workflow-derived interest for candidate discovery.
//!
//! Open discovery remains level-triggered from all declared workflow labels.
//! Terminal discovery is opt-in: only queues declaring `terminal: true`
//! contribute positive labels. Historical states, exclusions, transition
//! effects, and gate labels never become periodic terminal interest by merely
//! existing in the workflow.

use crate::{
    ArtifactSource, ArtifactTarget, ClassifiedArtifact, Classifier, GateCondition, LabelId,
    ValidatedWorkflow, WorkflowMetadata, matches_queue_cheap, parse_metadata_block,
    requires_human_attention,
};
use temper_forge::{Issue, PullRequest};

/// Platform-owned durable evidence recovered independently of ordinary queue
/// label history.
///
/// These evidence classes are exact durable records or explicitly declared
/// recovery mechanisms. They are not inferred from every historical workflow
/// label and therefore do not widen periodic terminal candidate queries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlatformRecoveryEvidence {
    IncompleteJournal,
    DurableAssignment,
    DurableLease,
    ProviderOrCiRecovery,
    IncompleteFanOut,
    DependencyGate,
    DeclaredPostMergeQueue,
}

/// Durable recovery classes owned by the runtime rather than queue history.
pub const PLATFORM_DURABLE_RECOVERY_EVIDENCE: &[PlatformRecoveryEvidence] = &[
    PlatformRecoveryEvidence::IncompleteJournal,
    PlatformRecoveryEvidence::DurableAssignment,
    PlatformRecoveryEvidence::DurableLease,
    PlatformRecoveryEvidence::ProviderOrCiRecovery,
    PlatformRecoveryEvidence::IncompleteFanOut,
    PlatformRecoveryEvidence::DependencyGate,
    PlatformRecoveryEvidence::DeclaredPostMergeQueue,
];

/// Workflow-wide candidate-discovery interest.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WorkflowInterest {
    targets: Vec<ArtifactTarget>,
    open_labels: Vec<String>,
    issue_terminal_labels: Vec<String>,
    pull_request_terminal_labels: Vec<String>,
}

impl WorkflowInterest {
    /// Derives deterministic interest from a validated workflow.
    pub fn from_workflow(workflow: &ValidatedWorkflow) -> Self {
        let targets = workflow_targets(workflow);
        let open_labels = workflow
            .labels()
            .iter()
            .map(|label| label.as_str().to_string())
            .collect();
        let issue_terminal_labels = targets
            .contains(&ArtifactTarget::Issue)
            .then(|| terminal_labels(workflow, ArtifactTarget::Issue))
            .unwrap_or_default();
        let pull_request_terminal_labels = targets
            .contains(&ArtifactTarget::PullRequest)
            .then(|| terminal_labels(workflow, ArtifactTarget::PullRequest))
            .unwrap_or_default();
        Self {
            targets,
            open_labels,
            issue_terminal_labels,
            pull_request_terminal_labels,
        }
    }

    /// Forge artifact targets represented by at least one artifact kind.
    pub fn targets(&self) -> &[ArtifactTarget] {
        &self.targets
    }

    /// Whether the workflow represents `target`.
    pub fn has_target(&self, target: ArtifactTarget) -> bool {
        self.targets.contains(&target)
    }

    /// Every declared workflow label, in declaration order.
    pub fn open_labels(&self) -> &[String] {
        &self.open_labels
    }

    /// Explicit, positive terminal/recovery labels for one Forge target.
    pub fn terminal_labels(&self, target: ArtifactTarget) -> &[String] {
        match target {
            ArtifactTarget::Issue => &self.issue_terminal_labels,
            ArtifactTarget::PullRequest => &self.pull_request_terminal_labels,
        }
    }
}

/// Derives the shared candidate-discovery interest for `workflow`.
pub fn workflow_interest(workflow: &ValidatedWorkflow) -> WorkflowInterest {
    WorkflowInterest::from_workflow(workflow)
}

/// Returns whether a terminal issue carries explicit queue or durable recovery
/// interest. This predicate intentionally uses only candidate-summary fields;
/// callers can apply it before dependency, CI, review, comment, relation, or
/// exact-detail reads.
pub fn terminal_issue_recovery_interest(workflow: &ValidatedWorkflow, issue: &Issue) -> bool {
    terminal_recovery_interest(
        workflow,
        ArtifactSource::Issue {
            number: issue.number,
        },
        &issue.labels,
        &issue.body,
        &issue.dependencies,
    )
}

/// Pull-request counterpart of [`terminal_issue_recovery_interest`].
pub fn terminal_pull_request_recovery_interest(
    workflow: &ValidatedWorkflow,
    pull_request: &PullRequest,
) -> bool {
    terminal_recovery_interest(
        workflow,
        ArtifactSource::PullRequest {
            number: pull_request.number,
        },
        &pull_request.labels,
        &pull_request.body,
        &pull_request.dependencies,
    )
}

fn terminal_recovery_interest(
    workflow: &ValidatedWorkflow,
    source: ArtifactSource,
    labels: &[String],
    body: &str,
    dependencies: &[temper_forge::ItemNumber],
) -> bool {
    let metadata = match parse_metadata_block(body) {
        Ok(metadata) => metadata.unwrap_or_default(),
        Err(_) => return false,
    };
    if metadata.staged {
        return false;
    }
    // Interrupted-CI parking owns its attention barrier and must be able to
    // finish exact cleanup. Every unrelated needs-human artifact stays inert.
    if requires_human_attention(labels) && metadata.interrupted_ci_recovery.is_none() {
        return false;
    }
    if metadata_has_durable_recovery(&metadata) {
        return true;
    }

    let Ok(classified) = Classifier::new(workflow).classify_snapshot_with_dependencies(
        source,
        labels,
        body,
        dependencies,
    ) else {
        return false;
    };
    workflow
        .queues()
        .iter()
        .any(|queue| queue.terminal && matches_queue_cheap(queue, &classified))
        || dependency_recovery_declared(workflow, &classified)
}

fn metadata_has_durable_recovery(metadata: &WorkflowMetadata) -> bool {
    metadata.assignment.is_some()
        || metadata.lease.is_some()
        || metadata.provider_recovery.is_some()
        || metadata.missing_ci_recovery.is_some()
        || metadata.interrupted_ci_recovery.is_some()
        || metadata
            .create_issue_intents
            .values()
            .any(|intent| !intent.completed)
}

fn dependency_recovery_declared(
    workflow: &ValidatedWorkflow,
    artifact: &ClassifiedArtifact,
) -> bool {
    !artifact.relations.is_empty()
        && workflow.transitions().iter().any(|transition| {
            transition.artifact == artifact.kind
                && transition.requires_gates.iter().any(|required| {
                    workflow.gates().iter().any(|gate| {
                        gate.id == *required
                            && matches!(gate.condition, Some(GateCondition::DependenciesResolved))
                    })
                })
        })
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

fn terminal_labels(workflow: &ValidatedWorkflow, target: ArtifactTarget) -> Vec<String> {
    let mut interest = Vec::<LabelId>::new();
    for queue in workflow.queues().iter().filter(|queue| queue.terminal) {
        let queue_targets_target = queue.artifacts.iter().any(|artifact| {
            workflow
                .artifact_kind(artifact)
                .is_some_and(|kind| kind.target == target)
        });
        if !queue_targets_target {
            continue;
        }

        let has_complete_positive_evidence = !queue.labels.is_empty()
            || (!queue.any_of.is_empty()
                && queue.any_of.iter().all(|branch| !branch.labels.is_empty()));
        let mut positive = Vec::new();
        if has_complete_positive_evidence {
            for label in &queue.labels {
                push_label(&mut positive, label);
            }
            for branch in &queue.any_of {
                for label in &branch.labels {
                    push_label(&mut positive, label);
                }
            }
        } else {
            // A condition-only queue or empty any-of branch has no complete
            // positive projection. Validation guarantees every selected kind
            // then has identifying labels, which form the narrowest portable
            // discovery fallback without dropping the unlabelled branch.
            for artifact in &queue.artifacts {
                let Some(kind) = workflow.artifact_kind(artifact) else {
                    continue;
                };
                if kind.target == target {
                    for label in &kind.identifying_labels {
                        push_label(&mut positive, label);
                    }
                }
            }
        }
        for label in positive {
            push_label(&mut interest, &label);
        }
    }

    workflow
        .labels()
        .iter()
        .filter(|label| interest.contains(label))
        .map(|label| label.as_str().to_string())
        .collect()
}

fn push_label(labels: &mut Vec<LabelId>, label: &LabelId) {
    if !labels.contains(label) {
        labels.push(label.clone());
    }
}
