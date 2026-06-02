use super::{EnsureOutcome, ExecutionError, Executor};
use crate::artifact::ArtifactRef;
use crate::metadata::{parse_metadata_block, replace_metadata_block, WorkflowMetadata};
use std::collections::BTreeSet;
use std::sync::{Condvar, Mutex, OnceLock};
use temper_forge::{
    CreateIssue, CreatePullRequest, Forge, Issue, IssueQuery, PullRequest, PullRequestQuery,
    RepositoryId, UpdateIssue,
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
        let _guard = correlation_locks().acquire(lock_key("issue", repo_id, correlation_key));
        if let Some(existing) = self
            .find_issue_by_correlation(repo_id, correlation_key)
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
        let _guard = correlation_locks().acquire(lock_key("pull", repo_id, correlation_key));
        if let Some(existing) = self
            .find_pull_request_by_correlation(repo_id, correlation_key)
            .await?
        {
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
    async fn find_issue_by_correlation(
        &self,
        repo_id: &RepositoryId,
        correlation_key: &str,
    ) -> Result<Option<Issue>, ExecutionError> {
        let issues = self
            .forge
            .list_issues(repo_id, IssueQuery::default())
            .await?;
        Ok(issues
            .into_iter()
            .find(|issue| metadata_has_correlation_key(&issue.body, correlation_key)))
    }

    async fn ensure_issue_parent(
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

    /// Finds a pull request whose metadata block carries the correlation key.
    async fn find_pull_request_by_correlation(
        &self,
        repo_id: &RepositoryId,
        correlation_key: &str,
    ) -> Result<Option<PullRequest>, ExecutionError> {
        let pull_requests = self
            .forge
            .list_pull_requests(repo_id, PullRequestQuery::default())
            .await?;
        Ok(pull_requests
            .into_iter()
            .find(|pull_request| metadata_has_correlation_key(&pull_request.body, correlation_key)))
    }
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
    held: Mutex<BTreeSet<String>>,
    changed: Condvar,
}

impl CorrelationLocks {
    fn acquire(&self, key: String) -> CorrelationGuard<'_> {
        let mut held = self.held.lock().expect("correlation lock poisoned");
        while held.contains(&key) {
            held = self.changed.wait(held).expect("correlation lock poisoned");
        }
        held.insert(key.clone());
        CorrelationGuard { locks: self, key }
    }
}

struct CorrelationGuard<'a> {
    locks: &'a CorrelationLocks,
    key: String,
}

impl Drop for CorrelationGuard<'_> {
    fn drop(&mut self) {
        let mut held = self.locks.held.lock().expect("correlation lock poisoned");
        held.remove(&self.key);
        self.locks.changed.notify_all();
    }
}

fn correlation_locks() -> &'static CorrelationLocks {
    static LOCKS: OnceLock<CorrelationLocks> = OnceLock::new();
    LOCKS.get_or_init(|| CorrelationLocks {
        held: Mutex::new(BTreeSet::new()),
        changed: Condvar::new(),
    })
}
