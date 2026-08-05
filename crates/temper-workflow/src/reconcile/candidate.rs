use super::{
    ArtifactSnapshot, ReconcileError, Reconciler, ReconciliationDetailCache,
    ReconciliationDetailCacheStats, RecoveryPolicy,
};
use crate::classify::Classifier;
use crate::dependency_state::DependencyStateIndex;
use crate::ids::ArtifactKindId;
use crate::validated::{GateCondition, ValidatedTransition, ValidatedWorkflow};
use crate::{ArtifactTarget, workflow_interest};
use std::time::Instant;
use temper_forge::{
    CandidateLabelSelection, CandidateLifecycle, CandidatePageRequest, Forge, Issue,
    IssueCandidateQuery, ItemListDetails, PullRequest, PullRequestCandidateQuery, RepositoryId,
};

pub(crate) struct CandidateLoad {
    pub(crate) snapshots: Vec<ArtifactSnapshot>,
    pub(crate) state_index: DependencyStateIndex,
    pub(crate) cache_stats: ReconciliationDetailCacheStats,
}

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
        Ok(self
            .load_bounded_candidate_snapshots_inner(forge, repo_id, chrono::Utc::now(), None)
            .await?
            .snapshots)
    }

    pub(crate) async fn load_bounded_candidate_snapshots_inner<F>(
        &self,
        forge: &F,
        repo_id: &RepositoryId,
        now: chrono::DateTime<chrono::Utc>,
        cache: Option<&ReconciliationDetailCache>,
    ) -> Result<CandidateLoad, ReconcileError>
    where
        F: Forge + ?Sized,
    {
        let plan = self.candidate_query_plan();
        let (issue_candidates, pull_request_candidates) =
            read_reconciliation_candidate_summaries(forge, repo_id, &plan).await?;
        self.load_candidate_snapshots_from_items(
            forge,
            repo_id,
            issue_candidates,
            pull_request_candidates,
            now,
            cache,
            &[],
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn load_candidate_snapshots_from_items<F>(
        &self,
        forge: &F,
        repo_id: &RepositoryId,
        mut issue_candidates: Vec<Issue>,
        mut pull_request_candidates: Vec<PullRequest>,
        now: chrono::DateTime<chrono::Utc>,
        cache: Option<&ReconciliationDetailCache>,
        forced_sources: &[crate::ArtifactSource],
    ) -> Result<CandidateLoad, ReconcileError>
    where
        F: Forge + ?Sized,
    {
        let state_index =
            DependencyStateIndex::from_candidates(&issue_candidates, &pull_request_candidates);
        issue_candidates.retain(|issue| {
            let source = crate::ArtifactSource::Issue {
                number: issue.number,
            };
            issue.state == temper_forge::IssueState::Open
                || forced_sources.contains(&source)
                || crate::terminal_issue_recovery_interest(self.workflow, issue)
        });
        pull_request_candidates.retain(|pull_request| {
            let source = crate::ArtifactSource::PullRequest {
                number: pull_request.number,
            };
            pull_request.state == temper_forge::PullRequestState::Open
                || forced_sources.contains(&source)
                || crate::terminal_pull_request_recovery_interest(self.workflow, pull_request)
        });
        let classifier = Classifier::new(self.workflow);
        let mut cache_stats = ReconciliationDetailCacheStats::default();
        if let Some(cache) = cache {
            cache.begin_pass(now, &mut cache_stats);
        }

        let mut snapshots = Vec::new();
        for issue in issue_candidates {
            if let Some(snapshot) = self
                .snapshot_for_issue_candidate(
                    forge,
                    repo_id,
                    &classifier,
                    issue,
                    now,
                    cache,
                    &mut cache_stats,
                )
                .await?
            {
                snapshots.push(snapshot);
            }
        }
        for pull_request in pull_request_candidates {
            if let Some(snapshot) = self
                .snapshot_for_pull_request_candidate(
                    forge,
                    repo_id,
                    &classifier,
                    pull_request,
                    now,
                    cache,
                    &mut cache_stats,
                )
                .await?
            {
                snapshots.push(snapshot);
            }
        }
        Ok(CandidateLoad {
            snapshots,
            state_index,
            cache_stats,
        })
    }

    async fn snapshot_for_issue_candidate<F>(
        &self,
        forge: &F,
        repo_id: &RepositoryId,
        classifier: &Classifier<'_>,
        mut issue: Issue,
        now: chrono::DateTime<chrono::Utc>,
        cache: Option<&ReconciliationDetailCache>,
        stats: &mut ReconciliationDetailCacheStats,
    ) -> Result<Option<ArtifactSnapshot>, ReconcileError>
    where
        F: Forge + ?Sized,
    {
        if !candidate_is_mechanically_visible(&issue.labels, &issue.body) {
            return Ok(Some(ArtifactSnapshot::from_issue(&issue)));
        }
        if self.issue_candidate_needs_dependency_detail(classifier, &issue) {
            if let Some(dependencies) =
                cache.and_then(|cache| cache.issue_dependencies(repo_id, &issue, now, stats))
            {
                issue.dependencies = dependencies;
                return Ok(Some(ArtifactSnapshot::from_issue(&issue)));
            }
            let exact = forge.get_issue_by_number(repo_id, issue.number).await?;
            if let Some(exact) = &exact {
                if let Some(cache) = cache {
                    stats.add_evictions(cache.store_issue_dependencies(
                        repo_id,
                        &issue,
                        exact.dependencies.clone(),
                        now,
                    ));
                }
            } else if let Some(cache) = cache {
                stats.add_invalidations(cache.invalidate(
                    repo_id,
                    crate::ArtifactSource::Issue {
                        number: issue.number,
                    },
                ));
            }
            return Ok(exact.as_ref().map(ArtifactSnapshot::from_issue));
        }
        Ok(Some(ArtifactSnapshot::from_issue(&issue)))
    }

    async fn snapshot_for_pull_request_candidate<F>(
        &self,
        forge: &F,
        repo_id: &RepositoryId,
        classifier: &Classifier<'_>,
        mut pull_request: PullRequest,
        now: chrono::DateTime<chrono::Utc>,
        cache: Option<&ReconciliationDetailCache>,
        stats: &mut ReconciliationDetailCacheStats,
    ) -> Result<Option<ArtifactSnapshot>, ReconcileError>
    where
        F: Forge + ?Sized,
    {
        if !candidate_is_mechanically_visible(&pull_request.labels, &pull_request.body) {
            return Ok(Some(ArtifactSnapshot::from_pull_request(&pull_request)));
        }
        if self.pull_request_candidate_needs_dependency_detail(classifier, &pull_request) {
            if let Some(dependencies) = cache.and_then(|cache| {
                cache.pull_request_dependencies(repo_id, &pull_request, now, stats)
            }) {
                pull_request.dependencies = dependencies;
                return Ok(Some(ArtifactSnapshot::from_pull_request(&pull_request)));
            }
            let exact = forge
                .get_pull_request_by_number(repo_id, pull_request.number)
                .await?;
            if let Some(exact) = &exact {
                if let Some(cache) = cache {
                    stats.add_evictions(cache.store_pull_request_dependencies(
                        repo_id,
                        &pull_request,
                        exact.dependencies.clone(),
                        now,
                    ));
                }
            } else if let Some(cache) = cache {
                stats.add_invalidations(cache.invalidate(
                    repo_id,
                    crate::ArtifactSource::PullRequest {
                        number: pull_request.number,
                    },
                ));
            }
            return Ok(exact.as_ref().map(ArtifactSnapshot::from_pull_request));
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

async fn read_reconciliation_candidate_summaries<F: Forge + ?Sized>(
    forge: &F,
    repo_id: &RepositoryId,
    plan: &ReconciliationCandidateQueryPlan,
) -> Result<(Vec<Issue>, Vec<PullRequest>), ReconcileError> {
    let started = Instant::now();
    let provider_requests_before = forge.provider_request_count();
    let logical_bucket_count = plan
        .issue_queries
        .len()
        .saturating_add(plan.pull_request_queries.len());
    let mut issues = Vec::new();
    let mut pull_requests = Vec::new();
    let result: Result<(), ReconcileError> = async {
        for query in &plan.issue_queries {
            issues.extend(forge.list_issue_candidates(repo_id, query.clone()).await?);
        }
        for query in &plan.pull_request_queries {
            pull_requests.extend(
                forge
                    .list_pull_request_candidates(repo_id, query.clone())
                    .await?,
            );
        }
        Ok(())
    }
    .await;

    normalize_issue_candidates(&mut issues);
    normalize_pull_request_candidates(&mut pull_requests);
    let unique_count = issues.len().saturating_add(pull_requests.len());
    let provider_requests = provider_requests_before.and_then(|before| {
        forge
            .provider_request_count()
            .map(|after| after.saturating_sub(before))
    });
    let outcome = if result.is_ok() { "success" } else { "failed" };
    tracing::debug!(
        target: "temper::worker",
        measurement = "candidate.discovery",
        repo = repository_label(repo_id),
        candidate.consumer = "mechanical",
        candidate.scope = "reconciliation",
        candidate.logical_bucket_count = saturating_u64(logical_bucket_count),
        candidate.provider_request_total = provider_requests.unwrap_or(0),
        candidate.provider_requests_available = provider_requests.is_some(),
        candidate.unique_count = saturating_u64(unique_count),
        outcome,
        duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        "candidate discovery {outcome} for mechanical/reconciliation"
    );
    result?;
    Ok((issues, pull_requests))
}

fn normalize_issue_candidates(issues: &mut Vec<Issue>) {
    issues.sort_by(|left, right| {
        left.number
            .cmp(&right.number)
            .then_with(|| left.id.cmp(&right.id))
    });
    issues.dedup_by(|left, right| left.id == right.id);
}

fn normalize_pull_request_candidates(pull_requests: &mut Vec<PullRequest>) {
    pull_requests.sort_by(|left, right| {
        left.number
            .cmp(&right.number)
            .then_with(|| left.id.cmp(&right.id))
    });
    pull_requests.dedup_by(|left, right| left.id == right.id);
}

fn repository_label(repo_id: &RepositoryId) -> &str {
    match repo_id.as_str().split_once(':') {
        Some((_provider, path)) if path.contains('/') => path,
        _ => repo_id.as_str(),
    }
}

fn saturating_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

fn candidate_is_mechanically_visible(labels: &[String], body: &str) -> bool {
    !crate::requires_human_attention(labels)
        && !crate::parse_metadata_block(body)
            .ok()
            .flatten()
            .is_some_and(|metadata| metadata.staged)
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
        page: (lifecycle == CandidateLifecycle::Terminal).then(CandidatePageRequest::terminal),
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
        page: (lifecycle == CandidateLifecycle::Terminal).then(CandidatePageRequest::terminal),
    }
}
