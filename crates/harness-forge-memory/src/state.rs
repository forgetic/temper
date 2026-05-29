//! In-memory record store backing [`MemoryForge`](crate::MemoryForge).
//!
//! `State` holds every record kind in ordinary collections guarded by a single
//! mutex (see [`crate`]). It owns the deterministic logical clock and the
//! repository-id counter, mirroring the filesystem backend so the two reference
//! backends produce identical ids and timestamps for the same call sequence.

use crate::ids::repository_id;
use chrono::{DateTime, Utc};
use harness_forge::{
    CiJob, Comment, ForgeError, ForgeResult, Issue, IssueId, PullRequest, PullRequestId,
    Repository, RepositoryId, RepositoryPath, User, UserId,
};
use std::collections::BTreeMap;

const DEFAULT_USER_ID: &str = "user-1";
const DEFAULT_USER_HANDLE: &str = "local";
const DEFAULT_USER_DISPLAY_NAME: &str = "Local User";

/// The full in-memory record set for one backend instance.
#[derive(Clone, Debug)]
pub(crate) struct State {
    pub(crate) current_user: User,
    clock_tick: u64,
    next_repository_number: u64,
    repositories: Vec<Repository>,
    labels: BTreeMap<String, Vec<harness_forge::Label>>,
    issues: BTreeMap<String, Vec<Issue>>,
    pull_requests: BTreeMap<String, Vec<PullRequest>>,
    issue_comments: BTreeMap<String, Vec<Comment>>,
    pull_request_comments: BTreeMap<String, Vec<Comment>>,
    pull_request_reviews: BTreeMap<String, Vec<harness_forge::PullRequestReview>>,
    ci_jobs: BTreeMap<String, Vec<CiJob>>,
}

impl State {
    /// Creates an empty store with the given bootstrapped current user.
    pub(crate) fn new(current_user: User) -> Self {
        Self {
            current_user,
            clock_tick: 0,
            next_repository_number: 1,
            repositories: Vec::new(),
            labels: BTreeMap::new(),
            issues: BTreeMap::new(),
            pull_requests: BTreeMap::new(),
            issue_comments: BTreeMap::new(),
            pull_request_comments: BTreeMap::new(),
            pull_request_reviews: BTreeMap::new(),
            ci_jobs: BTreeMap::new(),
        }
    }

    /// The default bootstrapped current user, matching the filesystem backend.
    pub(crate) fn default_user() -> User {
        User {
            id: UserId::new(DEFAULT_USER_ID),
            handle: DEFAULT_USER_HANDLE.into(),
            display_name: Some(DEFAULT_USER_DISPLAY_NAME.into()),
            email: None,
        }
    }

    /// Advances the logical clock by one second and returns the new timestamp.
    pub(crate) fn next_timestamp(&mut self) -> ForgeResult<DateTime<Utc>> {
        self.clock_tick = self
            .clock_tick
            .checked_add(1)
            .ok_or_else(|| ForgeError::Backend("in-memory logical clock overflowed".into()))?;
        timestamp_from_tick(self.clock_tick)
    }

    /// Returns the current logical clock tick.
    pub(crate) fn clock_tick(&self) -> u64 {
        self.clock_tick
    }

    /// Allocates the next unused deterministic repository id.
    pub(crate) fn allocate_repository_id(&mut self) -> ForgeResult<RepositoryId> {
        loop {
            let number = self.next_repository_number;
            self.next_repository_number = number
                .checked_add(1)
                .ok_or_else(|| ForgeError::Backend("repository id counter overflowed".into()))?;
            let id = repository_id(number);
            if !self.repositories.iter().any(|repo| repo.id == id) {
                return Ok(id);
            }
        }
    }

    pub(crate) fn repositories(&self) -> Vec<Repository> {
        self.repositories.clone()
    }

    pub(crate) fn insert_repository(&mut self, repository: Repository) {
        self.repositories.push(repository);
    }

    pub(crate) fn find_repository_by_id(&self, id: &RepositoryId) -> Option<Repository> {
        self.repositories
            .iter()
            .find(|repo| &repo.id == id)
            .cloned()
    }

    pub(crate) fn repository_exists(&self, id: &RepositoryId) -> bool {
        self.repositories.iter().any(|repo| &repo.id == id)
    }

    pub(crate) fn require_repository(&self, id: &RepositoryId) -> ForgeResult<()> {
        if self.repository_exists(id) {
            Ok(())
        } else {
            Err(ForgeError::NotFound(format!("repository {id}")))
        }
    }

    pub(crate) fn find_repository_by_path(&self, path: &RepositoryPath) -> Option<Repository> {
        self.repositories
            .iter()
            .find(|repo| repo.owner == path.owner && repo.name == path.name)
            .cloned()
    }

    pub(crate) fn labels_mut(&mut self, repo_id: &RepositoryId) -> &mut Vec<harness_forge::Label> {
        self.labels.entry(repo_id.as_str().to_string()).or_default()
    }

    pub(crate) fn labels(&self, repo_id: &RepositoryId) -> Vec<harness_forge::Label> {
        self.labels
            .get(repo_id.as_str())
            .cloned()
            .unwrap_or_default()
    }

    pub(crate) fn issues_mut(&mut self, repo_id: &RepositoryId) -> &mut Vec<Issue> {
        self.issues.entry(repo_id.as_str().to_string()).or_default()
    }

    pub(crate) fn issues(&self, repo_id: &RepositoryId) -> Vec<Issue> {
        self.issues
            .get(repo_id.as_str())
            .cloned()
            .unwrap_or_default()
    }

    /// Finds the issue and its owning repository id by issue id.
    pub(crate) fn find_issue(&self, id: &IssueId) -> Option<(RepositoryId, Issue)> {
        for (repo, issues) in &self.issues {
            if let Some(issue) = issues.iter().find(|issue| &issue.id == id) {
                return Some((RepositoryId::new(repo.clone()), issue.clone()));
            }
        }
        None
    }

    pub(crate) fn pull_requests_mut(&mut self, repo_id: &RepositoryId) -> &mut Vec<PullRequest> {
        self.pull_requests
            .entry(repo_id.as_str().to_string())
            .or_default()
    }

    pub(crate) fn pull_requests(&self, repo_id: &RepositoryId) -> Vec<PullRequest> {
        self.pull_requests
            .get(repo_id.as_str())
            .cloned()
            .unwrap_or_default()
    }

    /// Finds the pull request and its owning repository id by pull-request id.
    pub(crate) fn find_pull_request(
        &self,
        id: &PullRequestId,
    ) -> Option<(RepositoryId, PullRequest)> {
        for (repo, pull_requests) in &self.pull_requests {
            if let Some(pull_request) = pull_requests.iter().find(|pr| &pr.id == id) {
                return Some((RepositoryId::new(repo.clone()), pull_request.clone()));
            }
        }
        None
    }

    pub(crate) fn issue_comments_mut(&mut self, id: &IssueId) -> &mut Vec<Comment> {
        self.issue_comments
            .entry(id.as_str().to_string())
            .or_default()
    }

    pub(crate) fn issue_comments(&self, id: &IssueId) -> Vec<Comment> {
        self.issue_comments
            .get(id.as_str())
            .cloned()
            .unwrap_or_default()
    }

    pub(crate) fn pull_request_comments_mut(&mut self, id: &PullRequestId) -> &mut Vec<Comment> {
        self.pull_request_comments
            .entry(id.as_str().to_string())
            .or_default()
    }

    pub(crate) fn pull_request_comments(&self, id: &PullRequestId) -> Vec<Comment> {
        self.pull_request_comments
            .get(id.as_str())
            .cloned()
            .unwrap_or_default()
    }

    pub(crate) fn pull_request_reviews_mut(
        &mut self,
        id: &PullRequestId,
    ) -> &mut Vec<harness_forge::PullRequestReview> {
        self.pull_request_reviews
            .entry(id.as_str().to_string())
            .or_default()
    }

    pub(crate) fn pull_request_reviews(
        &self,
        id: &PullRequestId,
    ) -> Vec<harness_forge::PullRequestReview> {
        self.pull_request_reviews
            .get(id.as_str())
            .cloned()
            .unwrap_or_default()
    }

    pub(crate) fn ci_jobs(&self, repo_id: &RepositoryId) -> Vec<CiJob> {
        self.ci_jobs
            .get(repo_id.as_str())
            .cloned()
            .unwrap_or_default()
    }

    pub(crate) fn set_ci_jobs(&mut self, repo_id: &RepositoryId, jobs: Vec<CiJob>) {
        self.ci_jobs.insert(repo_id.as_str().to_string(), jobs);
    }

    /// Finds a CI job by id across every repository.
    pub(crate) fn find_ci_job(&self, id: &harness_forge::CiJobId) -> Option<CiJob> {
        self.ci_jobs
            .values()
            .flat_map(|jobs| jobs.iter())
            .find(|job| &job.id == id)
            .cloned()
    }
}

fn timestamp_from_tick(tick: u64) -> ForgeResult<DateTime<Utc>> {
    let seconds = i64::try_from(tick)
        .map_err(|_| ForgeError::Backend("in-memory logical clock is too large".into()))?;
    DateTime::<Utc>::from_timestamp(seconds, 0)
        .ok_or_else(|| ForgeError::Backend("in-memory logical clock is out of range".into()))
}
