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
    consumer: &'static str,
    scope: &'static str,
    now: DateTime<Utc>,
    discovery: Option<(&TerminalDiscoveryState, TerminalDiscoveryRead)>,
    exact_targets: &[ArtifactAddress],
    include_filtered_terminal_summaries: bool,
) -> Result<(Vec<Issue>, Vec<PullRequest>), ScanError> {
    let started = Instant::now();
    let provider_requests_before = forge.provider_request_count();
    let logical_bucket_count = query_plan
        .issue_queries
        .len()
        .saturating_add(query_plan.pull_request_queries.len());
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
                    let terminal = issue.state == IssueState::Closed;
                    let actionable = !terminal
                        || temper_workflow::terminal_issue_recovery_interest(workflow, &issue);
                    if retained.contains(&target) && (!terminal || !actionable) {
                        discovery.remove_exact_target(repo, target);
                    }
                    if forced.contains(&target) || actionable {
                        issues.push(issue);
                    }
                }
                temper_forge::HintArtifactKind::PullRequest => {
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
                        pull_requests.push(pull_request);
                    }
                }
            }
        }

        for query in &query_plan.issue_queries {
            if query.lifecycle == CandidateLifecycle::Open {
                issues.extend(
                    forge
                        .list_issue_candidates(repo, query.clone())
                        .await?
                        .into_iter()
                        .filter(|issue| issue.state == IssueState::Open),
                );
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
            let page = match forge.list_issue_candidates(repo, query).await {
                Ok(page) => page,
                Err(error) => {
                    discovery
                        .record_failed_page(repo, &fingerprint, &bucket)
                        .map_err(discovery_error)?;
                    return Err(error.into());
                }
            };
            let retained_targets = page
                .items
                .iter()
                .filter(|issue| issue.state == IssueState::Closed)
                .filter(|issue| temper_workflow::terminal_issue_recovery_interest(workflow, issue))
                .map(|issue| ArtifactAddress::issue(issue.number))
                .collect::<Vec<_>>();
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
                pull_requests.extend(
                    forge
                        .list_pull_request_candidates(repo, query.clone())
                        .await?
                        .into_iter()
                        .filter(|pull_request| pull_request.state == PullRequestState::Open),
                );
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
            let page = match forge.list_pull_request_candidates(repo, query).await {
                Ok(page) => page,
                Err(error) => {
                    discovery
                        .record_failed_page(repo, &fingerprint, &bucket)
                        .map_err(discovery_error)?;
                    return Err(error.into());
                }
            };
            let retained_targets = page
                .items
                .iter()
                .filter(|pull_request| pull_request.state != PullRequestState::Open)
                .filter(|pull_request| {
                    temper_workflow::terminal_pull_request_recovery_interest(workflow, pull_request)
                })
                .map(|pull_request| ArtifactAddress::pull_request(pull_request.number))
                .collect::<Vec<_>>();
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
        repo = strip_provider_scheme(repo.as_str()),
        candidate.consumer = consumer,
        candidate.scope = scope,
        candidate.logical_bucket_count = saturating_u64(logical_bucket_count),
        candidate.provider_request_total = provider_requests.unwrap_or(0),
        candidate.provider_requests_available = provider_requests.is_some(),
        candidate.unique_count = saturating_u64(unique_count),
        outcome,
        duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        "candidate discovery {outcome} for {consumer}/{scope}"
    );
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
