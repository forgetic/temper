use super::{ArtifactSnapshot, ReconcileError, Reconciler, RecoveryPolicy};
use crate::classify::Classifier;
use crate::ids::ArtifactKindId;
use crate::validated::{GateCondition, ValidatedTransition, ValidatedWorkflow};
use crate::{ArtifactTarget, workflow_interest};
use temper_forge::{
    CandidateLabelSelection, CandidateLifecycle, Forge, Issue, IssueCandidateQuery,
    ItemListDetails, PullRequest, PullRequestCandidateQuery, RepositoryId,
};

/// Consolidated Forge candidate buckets used by bounded reconciliation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReconciliationCandidateQueryPlan {
    /// At most one open and one terminal issue bucket.
    pub issue_queries: Vec<IssueCandidateQuery>,
    /// At most one open and one terminal pull-request bucket.
    pub pull_request_queries: Vec<PullRequestCandidateQuery>,
}

/// Plans workflow-labelled candidate buckets from a validated workflow.
///
/// Open reconciliation is interested in any declared workflow label. Terminal
/// reconciliation uses the shared, target-specific recovery interest and never
/// emits an unfiltered terminal bucket.
pub fn reconciliation_candidate_query_plan(
    workflow: &ValidatedWorkflow,
) -> ReconciliationCandidateQueryPlan {
    let interest = workflow_interest(workflow);
    let mut plan = ReconciliationCandidateQueryPlan::default();

    if interest.has_target(ArtifactTarget::Issue) {
        if !interest.open_labels().is_empty() {
            plan.issue_queries.push(issue_candidate(
                CandidateLifecycle::Open,
                interest.open_labels().to_vec(),
            ));
        }
        if !interest.terminal_labels(ArtifactTarget::Issue).is_empty() {
            plan.issue_queries.push(issue_candidate(
                CandidateLifecycle::Terminal,
                interest.terminal_labels(ArtifactTarget::Issue).to_vec(),
            ));
        }
    }

    if interest.has_target(ArtifactTarget::PullRequest) {
        if !interest.open_labels().is_empty() {
            plan.pull_request_queries.push(pull_request_candidate(
                CandidateLifecycle::Open,
                interest.open_labels().to_vec(),
            ));
        }
        if !interest
            .terminal_labels(ArtifactTarget::PullRequest)
            .is_empty()
        {
            plan.pull_request_queries.push(pull_request_candidate(
                CandidateLifecycle::Terminal,
                interest
                    .terminal_labels(ArtifactTarget::PullRequest)
                    .to_vec(),
            ));
        }
    }
    plan
}

impl<P: RecoveryPolicy> Reconciler<'_, P> {
    /// Returns the workflow-labelled query plan used by bounded reconciliation.
    pub fn candidate_query_plan(&self) -> ReconciliationCandidateQueryPlan {
        reconciliation_candidate_query_plan(self.workflow)
    }

    /// Loads workflow-labelled reconciliation candidates with summary candidate
    /// reads and exact dependency detail only for dependency-gated kinds.
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
        let mut issue_candidates = Vec::new();
        for query in plan.issue_queries {
            issue_candidates.extend(forge.list_issue_candidates(repo_id, query).await?);
        }
        issue_candidates.sort_by(|left, right| {
            left.number
                .cmp(&right.number)
                .then_with(|| left.id.cmp(&right.id))
        });
        issue_candidates.dedup_by(|left, right| left.id == right.id);

        let mut pull_request_candidates = Vec::new();
        for query in plan.pull_request_queries {
            pull_request_candidates
                .extend(forge.list_pull_request_candidates(repo_id, query).await?);
        }
        pull_request_candidates.sort_by(|left, right| {
            left.number
                .cmp(&right.number)
                .then_with(|| left.id.cmp(&right.id))
        });
        pull_request_candidates.dedup_by(|left, right| left.id == right.id);

        let mut snapshots = Vec::new();
        for issue in issue_candidates {
            if let Some(snapshot) = self
                .snapshot_for_issue_candidate(forge, repo_id, &classifier, issue)
                .await?
            {
                snapshots.push(snapshot);
            }
        }
        for pull_request in pull_request_candidates {
            if let Some(snapshot) = self
                .snapshot_for_pull_request_candidate(forge, repo_id, &classifier, pull_request)
                .await?
            {
                snapshots.push(snapshot);
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

fn issue_candidate(lifecycle: CandidateLifecycle, labels: Vec<String>) -> IssueCandidateQuery {
    IssueCandidateQuery {
        lifecycle,
        labels: CandidateLabelSelection::AnyOf(labels),
        details: ItemListDetails::summary(),
    }
}

fn pull_request_candidate(
    lifecycle: CandidateLifecycle,
    labels: Vec<String>,
) -> PullRequestCandidateQuery {
    PullRequestCandidateQuery {
        lifecycle,
        labels: CandidateLabelSelection::AnyOf(labels),
        details: ItemListDetails::summary(),
    }
}
