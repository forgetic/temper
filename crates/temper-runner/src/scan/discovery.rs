//! Shared bounded candidate-page loading for role and mechanical consumers.

use super::candidate::CandidateQueryPlan;
use super::query::TerminalDiscoveryRead;
use super::{
    ArtifactAddress, ScanError, TerminalDiscoveryBucket, TerminalDiscoveryContinuation,
    TerminalDiscoveryPageCommit, TerminalDiscoverySnapshot, TerminalDiscoveryState,
};
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::time::Instant;
use temper_forge::{
    CandidateLifecycle, CandidatePageRequest, Forge, Issue, IssueCandidatePage, IssueState,
    ItemListDetails, PullRequest, PullRequestCandidatePage, PullRequestState, RepositoryId,
};
use temper_log::strip_provider_scheme;
use temper_workflow::ValidatedWorkflow;

/// Per-pass candidate cardinality and traversal measurement.
///
/// Provider rows may be duplicated across any-label streams, while only rows
/// that survive terminal recovery interest are retained for later hydration.
pub(super) struct CandidateDiscoveryMeasurement {
    started: Instant,
    provider_requests_before: Option<u64>,
    logical_bucket_count: usize,
    logical_query_count: usize,
    raw_provider_row_count: usize,
    unique_rows: BTreeSet<ArtifactAddress>,
    retained_rows: BTreeSet<ArtifactAddress>,
    hydrated_artifact_count: usize,
    exact_detail_read_count: usize,
    discovery_cache_reused: bool,
    continuation_bucket_count: usize,
    overflow_bucket_count: usize,
    completed_bucket_count: usize,
    discovery_complete: bool,
    retained_overflow: bool,
}

impl CandidateDiscoveryMeasurement {
    pub(super) fn new<F: Forge + ?Sized>(forge: &F, plan: &CandidateQueryPlan) -> Self {
        Self {
            started: Instant::now(),
            provider_requests_before: forge.provider_request_count(),
            logical_bucket_count: plan
                .issue_queries
                .len()
                .saturating_add(plan.pull_request_queries.len()),
            logical_query_count: 0,
            raw_provider_row_count: 0,
            unique_rows: BTreeSet::new(),
            retained_rows: BTreeSet::new(),
            hydrated_artifact_count: 0,
            exact_detail_read_count: 0,
            discovery_cache_reused: false,
            continuation_bucket_count: 0,
            overflow_bucket_count: 0,
            completed_bucket_count: 0,
            discovery_complete: false,
            retained_overflow: false,
        }
    }

    pub(super) fn record_exact_detail_read(&mut self) {
        self.exact_detail_read_count = self.exact_detail_read_count.saturating_add(1);
    }

    pub(super) fn record_hydrated_artifact(&mut self) {
        self.hydrated_artifact_count = self.hydrated_artifact_count.saturating_add(1);
    }

    pub(super) fn emit<F: Forge + ?Sized>(
        &self,
        forge: &F,
        repo: &RepositoryId,
        consumer: &'static str,
        scope: &'static str,
        success: bool,
    ) {
        let provider_requests = self.provider_requests_before.and_then(|before| {
            forge
                .provider_request_count()
                .map(|after| after.saturating_sub(before))
        });
        let outcome = if success { "success" } else { "failed" };
        tracing::debug!(
            target: "temper::worker",
            measurement = "candidate.discovery",
            repo = strip_provider_scheme(repo.as_str()),
            candidate.consumer = consumer,
            candidate.scope = scope,
            candidate.logical_bucket_count = saturating_u64(self.logical_bucket_count),
            candidate.logical_query_count = saturating_u64(self.logical_query_count),
            candidate.raw_provider_row_count = saturating_u64(self.raw_provider_row_count),
            candidate.unique_count = saturating_u64(self.unique_rows.len()),
            candidate.unique_row_count = saturating_u64(self.unique_rows.len()),
            candidate.retained_row_count = saturating_u64(self.retained_rows.len()),
            candidate.hydrated_artifact_count = saturating_u64(self.hydrated_artifact_count),
            candidate.exact_detail_read_count = saturating_u64(self.exact_detail_read_count),
            candidate.discovery_cache_reused = self.discovery_cache_reused,
            candidate.continuation_bucket_count = saturating_u64(self.continuation_bucket_count),
            candidate.overflow_bucket_count = saturating_u64(self.overflow_bucket_count),
            candidate.completed_bucket_count = saturating_u64(self.completed_bucket_count),
            candidate.discovery_complete = self.discovery_complete,
            candidate.retained_overflow = self.retained_overflow,
            candidate.provider_request_total = provider_requests.unwrap_or(0),
            candidate.provider_requests_available = provider_requests.is_some(),
            outcome,
            duration_ms = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX),
            "candidate discovery {outcome} for {consumer}/{scope}"
        );
    }

    fn record_query(&mut self) {
        self.logical_query_count = self.logical_query_count.saturating_add(1);
    }

    fn record_issue_page(&mut self, page: &IssueCandidatePage) {
        self.raw_provider_row_count = self.raw_provider_row_count.saturating_add(page.raw_count);
        self.unique_rows.extend(
            page.items
                .iter()
                .map(|issue| ArtifactAddress::issue(issue.number)),
        );
    }

    fn record_pull_request_page(&mut self, page: &PullRequestCandidatePage) {
        self.raw_provider_row_count = self.raw_provider_row_count.saturating_add(page.raw_count);
        self.unique_rows.extend(
            page.items
                .iter()
                .map(|pull_request| ArtifactAddress::pull_request(pull_request.number)),
        );
    }

    fn observe_snapshot(&mut self, snapshot: &TerminalDiscoverySnapshot) {
        self.discovery_cache_reused = snapshot.cache_reused;
        self.continuation_bucket_count = snapshot
            .buckets
            .values()
            .filter(|bucket| bucket.continuation.is_some())
            .count();
        self.overflow_bucket_count = snapshot
            .buckets
            .values()
            .filter(|bucket| bucket.overflow)
            .count();
        self.completed_bucket_count = snapshot
            .buckets
            .values()
            .filter(|bucket| bucket.complete)
            .count();
        self.discovery_complete = snapshot.authoritative;
        self.retained_overflow = snapshot.retained_overflow;
    }
}

pub(super) fn initialize_terminal_discovery(
    discovery: &TerminalDiscoveryState,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
    query_plan: &CandidateQueryPlan,
) -> Result<TerminalDiscoverySnapshot, ScanError> {
    discovery
        .begin(
            repo,
            workflow_fingerprint(workflow),
            terminal_buckets(query_plan)?,
        )
        .map_err(discovery_error)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn read_candidate_summaries<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
    query_plan: &CandidateQueryPlan,
    now: DateTime<Utc>,
    discovery: Option<(&TerminalDiscoveryState, TerminalDiscoveryRead)>,
    exact_targets: &[ArtifactAddress],
    include_filtered_terminal_summaries: bool,
    measurement: &mut CandidateDiscoveryMeasurement,
) -> Result<(Vec<Issue>, Vec<PullRequest>), ScanError> {
    let owned_state = discovery.is_none().then(TerminalDiscoveryState::default);
    let (discovery, terminal_read) = match discovery {
        Some(discovery) => discovery,
        None => (
            owned_state.as_ref().expect("local discovery state exists"),
            TerminalDiscoveryRead::Advance,
        ),
    };
    let fingerprint = workflow_fingerprint(workflow);
    let snapshot = initialize_terminal_discovery(discovery, repo, workflow, query_plan)?;
    measurement.observe_snapshot(&snapshot);
    let retained = snapshot
        .retained_targets
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let forced = exact_targets.iter().copied().collect::<BTreeSet<_>>();
    let exact = retained.union(&forced).copied().collect::<Vec<_>>();
    let mut issues = Vec::new();
    let mut pull_requests = Vec::new();

    let result: Result<(), ScanError> = async {
        for target in exact {
            match target.kind {
                temper_forge::HintArtifactKind::Issue => {
                    measurement.exact_detail_read_count =
                        measurement.exact_detail_read_count.saturating_add(1);
                    let issue = forge
                        .get_issue_by_number_with_details(
                            repo,
                            target.number,
                            ItemListDetails::summary(),
                        )
                        .await?;
                    let Some(issue) = issue else {
                        discovery.remove_exact_target(repo, target);
                        continue;
                    };
                    measurement.hydrated_artifact_count =
                        measurement.hydrated_artifact_count.saturating_add(1);
                    measurement.unique_rows.insert(target);
                    let terminal = issue.state == IssueState::Closed;
                    let actionable = !terminal
                        || temper_workflow::terminal_issue_recovery_interest(workflow, &issue);
                    if retained.contains(&target) && (!terminal || !actionable) {
                        discovery.remove_exact_target(repo, target);
                    }
                    if forced.contains(&target) || actionable {
                        measurement.retained_rows.insert(target);
                        issues.push(issue);
                    }
                }
                temper_forge::HintArtifactKind::PullRequest => {
                    measurement.exact_detail_read_count =
                        measurement.exact_detail_read_count.saturating_add(1);
                    let pull_request = forge
                        .get_pull_request_by_number_with_details(
                            repo,
                            target.number,
                            ItemListDetails::summary(),
                        )
                        .await?;
                    let Some(pull_request) = pull_request else {
                        discovery.remove_exact_target(repo, target);
                        continue;
                    };
                    measurement.hydrated_artifact_count =
                        measurement.hydrated_artifact_count.saturating_add(1);
                    measurement.unique_rows.insert(target);
                    let terminal = pull_request.state != PullRequestState::Open;
                    let actionable = !terminal
                        || temper_workflow::terminal_pull_request_recovery_interest(
                            workflow,
                            &pull_request,
                        );
                    if retained.contains(&target) && (!terminal || !actionable) {
                        discovery.remove_exact_target(repo, target);
                    }
                    if forced.contains(&target) || actionable {
                        measurement.retained_rows.insert(target);
                        pull_requests.push(pull_request);
                    }
                }
            }
        }

        for query in &query_plan.issue_queries {
            if query.lifecycle == CandidateLifecycle::Open {
                measurement.record_query();
                let page = forge.list_issue_candidates(repo, query.clone()).await?;
                measurement.record_issue_page(&page);
                let open = page
                    .into_iter()
                    .filter(|issue| issue.state == IssueState::Open)
                    .collect::<Vec<_>>();
                measurement.retained_rows.extend(
                    open.iter()
                        .map(|issue| ArtifactAddress::issue(issue.number)),
                );
                issues.extend(open);
                continue;
            }
            let bucket =
                TerminalDiscoveryBucket::issues(query.labels.clone()).map_err(discovery_error)?;
            let bucket_state = snapshot.buckets.get(&bucket).ok_or_else(|| {
                discovery_error_message("terminal issue bucket was not initialized")
            })?;
            if terminal_read == TerminalDiscoveryRead::RetainedOnly || bucket_state.complete {
                continue;
            }
            let mut query = query.clone();
            let request = query
                .page
                .get_or_insert_with(CandidatePageRequest::terminal);
            request.continuation = match bucket_state.continuation.clone() {
                Some(TerminalDiscoveryContinuation::Issue(continuation)) => Some(continuation),
                Some(TerminalDiscoveryContinuation::PullRequest(_)) => {
                    return Err(discovery_error_message(
                        "issue bucket carried a PR continuation",
                    ));
                }
                None => None,
            };
            measurement.record_query();
            let page = match forge.list_issue_candidates(repo, query).await {
                Ok(page) => page,
                Err(error) => {
                    discovery
                        .record_failed_page(repo, &fingerprint, &bucket)
                        .map_err(discovery_error)?;
                    return Err(error.into());
                }
            };
            measurement.record_issue_page(&page);
            let retained_targets = page
                .items
                .iter()
                .filter(|issue| issue.state == IssueState::Closed)
                .filter(|issue| temper_workflow::terminal_issue_recovery_interest(workflow, issue))
                .map(|issue| ArtifactAddress::issue(issue.number))
                .collect::<Vec<_>>();
            measurement
                .retained_rows
                .extend(retained_targets.iter().copied());
            let page_items = page
                .items
                .iter()
                .filter(|issue| issue.state == IssueState::Closed)
                .filter(|issue| {
                    include_filtered_terminal_summaries
                        || retained_targets.contains(&ArtifactAddress::issue(issue.number))
                })
                .cloned()
                .collect::<Vec<_>>();
            commit_issue_page(
                discovery,
                repo,
                &fingerprint,
                &bucket,
                bucket_state.sweep_boundary,
                now,
                page,
                retained_targets,
            )?;
            issues.extend(page_items);
        }

        for query in &query_plan.pull_request_queries {
            if query.lifecycle == CandidateLifecycle::Open {
                measurement.record_query();
                let page = forge
                    .list_pull_request_candidates(repo, query.clone())
                    .await?;
                measurement.record_pull_request_page(&page);
                let open = page
                    .into_iter()
                    .filter(|pull_request| pull_request.state == PullRequestState::Open)
                    .collect::<Vec<_>>();
                measurement.retained_rows.extend(
                    open.iter()
                        .map(|pull_request| ArtifactAddress::pull_request(pull_request.number)),
                );
                pull_requests.extend(open);
                continue;
            }
            let bucket = TerminalDiscoveryBucket::pull_requests(query.labels.clone())
                .map_err(discovery_error)?;
            let bucket_state = snapshot
                .buckets
                .get(&bucket)
                .ok_or_else(|| discovery_error_message("terminal PR bucket was not initialized"))?;
            if terminal_read == TerminalDiscoveryRead::RetainedOnly || bucket_state.complete {
                continue;
            }
            let mut query = query.clone();
            let request = query
                .page
                .get_or_insert_with(CandidatePageRequest::terminal);
            request.continuation = match bucket_state.continuation.clone() {
                Some(TerminalDiscoveryContinuation::PullRequest(continuation)) => {
                    Some(continuation)
                }
                Some(TerminalDiscoveryContinuation::Issue(_)) => {
                    return Err(discovery_error_message(
                        "PR bucket carried an issue continuation",
                    ));
                }
                None => None,
            };
            measurement.record_query();
            let page = match forge.list_pull_request_candidates(repo, query).await {
                Ok(page) => page,
                Err(error) => {
                    discovery
                        .record_failed_page(repo, &fingerprint, &bucket)
                        .map_err(discovery_error)?;
                    return Err(error.into());
                }
            };
            measurement.record_pull_request_page(&page);
            let retained_targets = page
                .items
                .iter()
                .filter(|pull_request| pull_request.state != PullRequestState::Open)
                .filter(|pull_request| {
                    temper_workflow::terminal_pull_request_recovery_interest(workflow, pull_request)
                })
                .map(|pull_request| ArtifactAddress::pull_request(pull_request.number))
                .collect::<Vec<_>>();
            measurement
                .retained_rows
                .extend(retained_targets.iter().copied());
            let page_items = page
                .items
                .iter()
                .filter(|pull_request| pull_request.state != PullRequestState::Open)
                .filter(|pull_request| {
                    include_filtered_terminal_summaries
                        || retained_targets
                            .contains(&ArtifactAddress::pull_request(pull_request.number))
                })
                .cloned()
                .collect::<Vec<_>>();
            commit_pull_request_page(
                discovery,
                repo,
                &fingerprint,
                &bucket,
                bucket_state.sweep_boundary,
                now,
                page,
                retained_targets,
            )?;
            pull_requests.extend(page_items);
        }
        Ok(())
    }
    .await;

    normalize_issue_candidates(&mut issues);
    normalize_pull_request_candidates(&mut pull_requests);
    if let Some(snapshot) = discovery.snapshot(repo) {
        let cache_reused = measurement.discovery_cache_reused;
        measurement.observe_snapshot(&snapshot);
        measurement.discovery_cache_reused = cache_reused;
    }
    result?;
    Ok((issues, pull_requests))
}

fn terminal_buckets(plan: &CandidateQueryPlan) -> Result<Vec<TerminalDiscoveryBucket>, ScanError> {
    let issues = plan
        .issue_queries
        .iter()
        .filter(|query| query.lifecycle == CandidateLifecycle::Terminal)
        .map(|query| TerminalDiscoveryBucket::issues(query.labels.clone()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(discovery_error)?;
    let pull_requests = plan
        .pull_request_queries
        .iter()
        .filter(|query| query.lifecycle == CandidateLifecycle::Terminal)
        .map(|query| TerminalDiscoveryBucket::pull_requests(query.labels.clone()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(discovery_error)?;
    Ok(issues.into_iter().chain(pull_requests).collect())
}

fn workflow_fingerprint(workflow: &ValidatedWorkflow) -> String {
    format!("sha256:{:x}", Sha256::digest(format!("{workflow:?}")))
}

fn commit_issue_page(
    discovery: &TerminalDiscoveryState,
    repo: &RepositoryId,
    fingerprint: &str,
    bucket: &TerminalDiscoveryBucket,
    prior_boundary: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    page: IssueCandidatePage,
    retained_targets: Vec<ArtifactAddress>,
) -> Result<(), ScanError> {
    let sweep_boundary = page
        .continuation
        .as_ref()
        .map(|continuation| continuation.boundary.updated_at)
        .or(prior_boundary)
        .or_else(|| page.items.iter().map(|issue| issue.updated_at).max())
        .or(Some(now));
    discovery
        .commit_page(
            repo,
            fingerprint,
            bucket,
            TerminalDiscoveryPageCommit {
                continuation: page.continuation.map(TerminalDiscoveryContinuation::Issue),
                exhausted: page.exhausted,
                overflow: page.overflow,
                sweep_boundary,
                retained_targets,
            },
        )
        .map_err(discovery_error)?;
    Ok(())
}

fn commit_pull_request_page(
    discovery: &TerminalDiscoveryState,
    repo: &RepositoryId,
    fingerprint: &str,
    bucket: &TerminalDiscoveryBucket,
    prior_boundary: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    page: PullRequestCandidatePage,
    retained_targets: Vec<ArtifactAddress>,
) -> Result<(), ScanError> {
    let sweep_boundary = page
        .continuation
        .as_ref()
        .map(|continuation| continuation.boundary.updated_at)
        .or(prior_boundary)
        .or_else(|| {
            page.items
                .iter()
                .map(|pull_request| pull_request.updated_at)
                .max()
        })
        .or(Some(now));
    discovery
        .commit_page(
            repo,
            fingerprint,
            bucket,
            TerminalDiscoveryPageCommit {
                continuation: page
                    .continuation
                    .map(TerminalDiscoveryContinuation::PullRequest),
                exhausted: page.exhausted,
                overflow: page.overflow,
                sweep_boundary,
                retained_targets,
            },
        )
        .map_err(discovery_error)?;
    Ok(())
}

fn discovery_error(error: impl std::fmt::Display) -> ScanError {
    discovery_error_message(&error.to_string())
}

fn discovery_error_message(message: &str) -> ScanError {
    ScanError::InvalidWorkflow(format!("terminal discovery state: {message}"))
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

fn saturating_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}
