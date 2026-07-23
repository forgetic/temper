use super::{EnsureOutcome, ExecutionError, Executor};
use crate::artifact::ArtifactRef;
use crate::metadata::{WorkflowMetadata, parse_metadata_block, replace_metadata_block};
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Mutex, OnceLock};
use std::task::{Context, Poll, Waker};
use temper_forge::{
    CreateIssue, CreatePullRequest, Forge, ForgeError, Issue, IssueId, IssueQuery, IssueState,
    ItemListDetails, PullRequest, PullRequestId, PullRequestQuery, PullRequestState, RepositoryId,
    UpdateIssue,
};

impl<F: Forge + ?Sized> Executor<'_, F> {
    /// Idempotently ensures an issue exists for a correlation key.
    ///
    /// Searches existing issues for one whose metadata block carries
    /// `correlation_key`; if found, returns it unchanged. Otherwise stamps the
    /// key into the new issue's metadata block and creates it. Retrying with the
    /// same key therefore returns the existing issue instead of duplicating it.
    pub async fn ensure_issue(
        &self,
        repo_id: &RepositoryId,
        correlation_key: &str,
        input: CreateIssue,
    ) -> Result<EnsureOutcome<Issue>, ExecutionError> {
        self.ensure_issue_with_parent(repo_id, correlation_key, None, input)
            .await
    }

    /// Idempotently ensures an issue exists for a correlation key and parent.
    ///
    /// This is the cross-repository fan-out variant used by runner role tools.
    /// It searches the target repository, creates when absent, and ensures an
    /// existing or newly created issue carries the repo-qualified parent
    /// back-reference in its metadata.
    pub async fn ensure_issue_with_parent(
        &self,
        repo_id: &RepositoryId,
        correlation_key: &str,
        parent: Option<ArtifactRef>,
        input: CreateIssue,
    ) -> Result<EnsureOutcome<Issue>, ExecutionError> {
        let _guard = correlation_locks()
            .acquire(lock_key("issue", repo_id, correlation_key))
            .await;
        if let Some(existing) = self
            .find_issue_by_correlation(repo_id, correlation_key, &input.labels)
            .await?
        {
            let existing = self.ensure_issue_parent(existing, parent).await?;
            return Ok(EnsureOutcome::Existing(existing));
        }

        let body =
            body_with_correlation_key_and_parent(&input.body, correlation_key, parent.as_ref())
                .map_err(|message| ExecutionError::Backend { message })?;
        let created = self
            .forge
            .create_issue(repo_id, CreateIssue { body, ..input })
            .await?;
        Ok(EnsureOutcome::Created(created))
    }

    /// Idempotently records a fallback dependency relation on an issue.
    ///
    /// Adds `dependency` to the issue's workflow metadata `dependencies` list
    /// under a compare-and-swap retry, returning `true` when the link was newly
    /// recorded and `false` when it was already present. This is the
    /// metadata-fallback form of an [ADR 0011] dependency relation, reused by
    /// `create_issues` to link sibling children (the cross-repo aggregation
    /// stance: non-atomic on real forges, idempotent across retries).
    pub async fn ensure_issue_dependency_metadata(
        &self,
        issue_id: &IssueId,
        dependency: &ArtifactRef,
    ) -> Result<bool, ExecutionError> {
        for _ in 0..3 {
            let issue =
                self.forge
                    .get_issue(issue_id)
                    .await?
                    .ok_or_else(|| ExecutionError::Backend {
                        message: format!("issue {issue_id:?} vanished while linking dependency"),
                    })?;
            let mut metadata = parse_metadata_block(&issue.body)
                .map_err(|error| ExecutionError::Backend {
                    message: format!("invalid issue workflow metadata: {error}"),
                })?
                .unwrap_or_default();
            if metadata
                .dependencies
                .iter()
                .any(|candidate| candidate == dependency)
            {
                return Ok(false);
            }
            metadata.dependencies.push(dependency.clone());
            let body = replace_metadata_block(&issue.body, &metadata).map_err(|error| {
                ExecutionError::Backend {
                    message: format!("could not update issue workflow metadata: {error}"),
                }
            })?;
            match self
                .forge
                .update_issue(
                    &issue.id,
                    UpdateIssue {
                        body: Some(body),
                        expected_version: Some(issue.version),
                        ..UpdateIssue::default()
                    },
                )
                .await
            {
                Ok(_) => return Ok(true),
                Err(ForgeError::Conflict(_)) => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(ExecutionError::Backend {
            message: format!(
                "could not add dependency metadata to issue {issue_id:?} after concurrent updates"
            ),
        })
    }

    /// Idempotently ensures a pull request exists for a correlation key.
    ///
    /// Searches existing pull requests for one whose metadata block carries
    /// `correlation_key`; if found, returns it unchanged. Otherwise stamps the
    /// key into the new pull request's metadata block and creates it. Retrying
    /// with the same key therefore returns the existing pull request instead of
    /// duplicating it.
    pub async fn ensure_pull_request(
        &self,
        repo_id: &RepositoryId,
        correlation_key: &str,
        input: CreatePullRequest,
    ) -> Result<EnsureOutcome<PullRequest>, ExecutionError> {
        let lookup_labels = input.labels.clone();
        self.ensure_pull_request_with_lookup(repo_id, correlation_key, &lookup_labels, input)
            .await
    }

    /// Like [`Executor::ensure_pull_request`], but the correlation lookup filters
    /// by `lookup_labels` instead of the create input's labels. Use when the
    /// created artifact carries creation-time labels that later transitions may
    /// remove (the lookup must key on the stable identifying labels).
    pub async fn ensure_pull_request_with_lookup(
        &self,
        repo_id: &RepositoryId,
        correlation_key: &str,
        lookup_labels: &[String],
        input: CreatePullRequest,
    ) -> Result<EnsureOutcome<PullRequest>, ExecutionError> {
        let _guard = correlation_locks()
            .acquire(lock_key("pull", repo_id, correlation_key))
            .await;
        if let Some(existing) = self
            .find_pull_request_by_correlation(repo_id, correlation_key, lookup_labels)
            .await?
        {
            validate_pull_request_topology(&existing, &input.source, &input.target)?;
            return Ok(EnsureOutcome::Existing(existing));
        }

        let body = body_with_correlation_key(&input.body, correlation_key)
            .map_err(|message| ExecutionError::Backend { message })?;
        let created = self
            .forge
            .create_pull_request(repo_id, CreatePullRequest { body, ..input })
            .await?;
        Ok(EnsureOutcome::Created(created))
    }

    /// Finds an issue whose metadata block carries the correlation key.
    pub(super) async fn find_issue_by_correlation(
        &self,
        repo_id: &RepositoryId,
        correlation_key: &str,
        labels: &[String],
    ) -> Result<Option<Issue>, ExecutionError> {
        let plan = CorrelationLookupPlan::new(correlation_key, labels);
        let mut seen = BTreeSet::<IssueId>::new();
        let mut candidates = Vec::new();
        for query in plan.issue_queries() {
            for issue in self.forge.list_issues(repo_id, query).await? {
                if seen.insert(issue.id.clone()) {
                    candidates.push(issue);
                }
            }
        }
        Ok(candidates
            .into_iter()
            .find(|issue| metadata_has_correlation_key(&issue.body, correlation_key)))
    }

    pub(super) async fn ensure_issue_parent(
        &self,
        issue: Issue,
        parent: Option<ArtifactRef>,
    ) -> Result<Issue, ExecutionError> {
        let Some(parent) = parent else {
            return Ok(issue);
        };
        if metadata_has_parent(&issue.body, &parent)? {
            return Ok(issue);
        }
        let body = body_with_parent(&issue.body, &parent)
            .map_err(|message| ExecutionError::Backend { message })?;
        Ok(self
            .forge
            .update_issue(
                &issue.id,
                UpdateIssue {
                    body: Some(body),
                    expected_version: Some(issue.version),
                    ..UpdateIssue::default()
                },
            )
            .await?)
    }

    /// Finds a pull request whose metadata block carries the correlation key,
    /// then reloads that candidate by number so callers receive fresh branch
    /// topology rather than a summary-list projection.
    pub async fn find_pull_request_by_correlation(
        &self,
        repo_id: &RepositoryId,
        correlation_key: &str,
        labels: &[String],
    ) -> Result<Option<PullRequest>, ExecutionError> {
        let Some(candidate) =
            find_pull_request_by_correlation(self.forge, repo_id, correlation_key, labels).await?
        else {
            return Ok(None);
        };
        self.forge
            .get_pull_request_by_number_with_details(
                repo_id,
                candidate.number,
                ItemListDetails::summary(),
            )
            .await?
            .map(Some)
            .ok_or_else(|| ExecutionError::Backend {
                message: format!(
                    "correlated pull request #{} vanished before topology validation",
                    candidate.number
                ),
            })
    }
}

/// Rejects reuse of an existing pull request whose branches differ from a
/// freshly resolved create input. Source and target repository identities are
/// part of the topology so a fork or cross-repository candidate cannot be
/// silently substituted either.
pub fn validate_pull_request_topology(
    existing: &PullRequest,
    expected_source: &temper_forge::BranchRef,
    expected_target: &temper_forge::BranchRef,
) -> Result<(), ExecutionError> {
    if existing.source == *expected_source && existing.target == *expected_target {
        return Ok(());
    }
    Err(ExecutionError::PullRequestTopologyMismatch {
        pull_request: existing.number,
        expected_source: Box::new(expected_source.clone()),
        expected_target: Box::new(expected_target.clone()),
        actual_source: Box::new(existing.source.clone()),
        actual_target: Box::new(existing.target.clone()),
    })
}

/// Finds a pull request whose metadata block carries the correlation key.
pub async fn find_pull_request_by_correlation<F: Forge + ?Sized>(
    forge: &F,
    repo_id: &RepositoryId,
    correlation_key: &str,
    labels: &[String],
) -> Result<Option<PullRequest>, ExecutionError> {
    let plan = CorrelationLookupPlan::new(correlation_key, labels);
    let mut seen = BTreeSet::<PullRequestId>::new();
    let mut candidates = Vec::new();
    for query in plan.pull_request_queries() {
        for pull_request in forge.list_pull_requests(repo_id, query).await? {
            if seen.insert(pull_request.id.clone()) {
                candidates.push(pull_request);
            }
        }
    }
    Ok(candidates
        .into_iter()
        .find(|pull_request| metadata_has_correlation_key(&pull_request.body, correlation_key)))
}

/// Bounded list-query plan for correlation-key lookups.
///
/// The body filter is only a narrowing hint. Callers must still parse the
/// workflow metadata block and compare the exact correlation key before
/// accepting a match.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CorrelationLookupPlan {
    body_contains: String,
    labels: Vec<String>,
}

impl CorrelationLookupPlan {
    /// Builds a state-explicit, summary-only correlation lookup plan.
    pub fn new(correlation_key: &str, labels: &[String]) -> Self {
        Self {
            body_contains: correlation_body_marker(correlation_key),
            labels: normalized_labels(labels),
        }
    }

    /// Issue queries for open and closed state, with configured labels when known.
    pub fn issue_queries(&self) -> Vec<IssueQuery> {
        [IssueState::Open, IssueState::Closed]
            .into_iter()
            .map(|state| IssueQuery {
                limit: None,
                state: Some(state),
                labels: self.labels.clone(),
                body_contains: Some(self.body_contains.clone()),
                author_id: None,
                assignee_id: None,
                sort: None,
                details: ItemListDetails::summary(),
            })
            .collect()
    }

    /// Pull-request queries for open, closed, and merged state, with configured labels when known.
    pub fn pull_request_queries(&self) -> Vec<PullRequestQuery> {
        [
            PullRequestState::Open,
            PullRequestState::Closed,
            PullRequestState::Merged,
        ]
        .into_iter()
        .map(|state| PullRequestQuery {
            limit: None,
            state: Some(state),
            labels: self.labels.clone(),
            body_contains: Some(self.body_contains.clone()),
            author_id: None,
            assignee_id: None,
            sort: None,
            details: ItemListDetails::summary(),
        })
        .collect()
    }
}

fn normalized_labels(labels: &[String]) -> Vec<String> {
    let mut labels = labels.to_vec();
    labels.sort();
    labels.dedup();
    labels
}

fn correlation_body_marker(correlation_key: &str) -> String {
    let escaped_key = serde_json::to_string(correlation_key)
        .expect("serializing a correlation key string cannot fail");
    format!("\"correlation_key\": {escaped_key}")
}

/// Returns `true` when `body` has a metadata block with `correlation_key`.
fn metadata_has_correlation_key(body: &str, correlation_key: &str) -> bool {
    matches!(
        parse_metadata_block(body),
        Ok(Some(WorkflowMetadata {
            correlation_key: Some(ref key),
            ..
        })) if key == correlation_key
    )
}

/// Returns `body` with `correlation_key` set in its metadata block.
///
/// Any existing metadata fields are preserved; only the correlation key is set.
/// The result round-trips through [`parse_metadata_block`], so a later search
/// can find the artifact.
fn body_with_correlation_key(body: &str, correlation_key: &str) -> Result<String, String> {
    body_with_correlation_key_and_parent(body, correlation_key, None)
}

fn body_with_correlation_key_and_parent(
    body: &str,
    correlation_key: &str,
    parent: Option<&ArtifactRef>,
) -> Result<String, String> {
    let mut metadata = parse_metadata_block(body)
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    metadata.correlation_key = Some(correlation_key.to_string());
    if let Some(parent) = parent {
        push_parent_once(&mut metadata, parent);
    }
    replace_metadata_block(body, &metadata).map_err(|error| error.to_string())
}

fn body_with_parent(body: &str, parent: &ArtifactRef) -> Result<String, String> {
    let mut metadata = parse_metadata_block(body)
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    push_parent_once(&mut metadata, parent);
    replace_metadata_block(body, &metadata).map_err(|error| error.to_string())
}

fn metadata_has_parent(body: &str, parent: &ArtifactRef) -> Result<bool, ExecutionError> {
    let metadata = parse_metadata_block(body).map_err(|error| ExecutionError::Backend {
        message: error.to_string(),
    })?;
    Ok(metadata
        .is_some_and(|metadata| metadata.parents.iter().any(|candidate| candidate == parent)))
}

fn push_parent_once(metadata: &mut WorkflowMetadata, parent: &ArtifactRef) {
    if !metadata.parents.iter().any(|candidate| candidate == parent) {
        metadata.parents.push(parent.clone());
    }
}

fn lock_key(kind: &str, repo_id: &RepositoryId, correlation_key: &str) -> String {
    format!(
        "{kind}:{}:{}:{}:{}",
        repo_id.as_str().len(),
        repo_id.as_str(),
        correlation_key.len(),
        correlation_key
    )
}

struct CorrelationLocks {
    state: Mutex<CorrelationLockState>,
}

#[derive(Default)]
struct CorrelationLockState {
    held: BTreeSet<String>,
    waiters: BTreeMap<String, Vec<Waker>>,
}

impl CorrelationLocks {
    fn new() -> Self {
        Self {
            state: Mutex::new(CorrelationLockState::default()),
        }
    }

    fn acquire(&self, key: String) -> CorrelationAcquire<'_> {
        CorrelationAcquire { locks: self, key }
    }
}

struct CorrelationAcquire<'a> {
    locks: &'a CorrelationLocks,
    key: String,
}

impl<'a> Future for CorrelationAcquire<'a> {
    type Output = CorrelationGuard<'a>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let mut state = this.locks.state.lock().expect("correlation lock poisoned");
        if !state.held.contains(&this.key) {
            state.held.insert(this.key.clone());
            return Poll::Ready(CorrelationGuard {
                locks: this.locks,
                key: this.key.clone(),
            });
        }

        state
            .waiters
            .entry(this.key.clone())
            .or_default()
            .push(cx.waker().clone());
        Poll::Pending
    }
}

struct CorrelationGuard<'a> {
    locks: &'a CorrelationLocks,
    key: String,
}

impl Drop for CorrelationGuard<'_> {
    fn drop(&mut self) {
        let waiters = {
            let mut state = self.locks.state.lock().expect("correlation lock poisoned");
            state.held.remove(&self.key);
            state.waiters.remove(&self.key).unwrap_or_default()
        };
        for waiter in waiters {
            waiter.wake();
        }
    }
}

fn correlation_locks() -> &'static CorrelationLocks {
    static LOCKS: OnceLock<CorrelationLocks> = OnceLock::new();
    LOCKS.get_or_init(CorrelationLocks::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::task::{Wake, Waker};

    #[test]
    fn correlation_lock_waits_without_blocking_the_executor_thread() {
        let locks = CorrelationLocks::new();
        let key = "pull:repo:run".to_string();

        let mut first = Box::pin(locks.acquire(key.clone()));
        let first_guard = match poll_acquire(first.as_mut()) {
            Poll::Ready(guard) => guard,
            Poll::Pending => panic!("uncontended acquire should be ready"),
        };

        let mut second = Box::pin(locks.acquire(key));
        assert!(matches!(poll_acquire(second.as_mut()), Poll::Pending));

        drop(first_guard);
        let second_guard = match poll_acquire(second.as_mut()) {
            Poll::Ready(guard) => guard,
            Poll::Pending => panic!("released key should wake the waiter"),
        };
        drop(second_guard);
    }

    fn poll_acquire<'a>(future: Pin<&mut CorrelationAcquire<'a>>) -> Poll<CorrelationGuard<'a>> {
        let waker = noop_waker();
        let mut context = Context::from_waker(&waker);
        Future::poll(future, &mut context)
    }

    fn noop_waker() -> Waker {
        struct NoopWake;
        impl Wake for NoopWake {
            fn wake(self: Arc<Self>) {}
        }
        Waker::from(Arc::new(NoopWake))
    }
}
