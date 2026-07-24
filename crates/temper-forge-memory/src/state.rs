//! In-memory record store backing [`MemoryForge`](crate::MemoryForge).
//!
//! `State` holds every record kind in ordinary collections guarded by a single
//! mutex (see [`crate`]). It owns the deterministic logical clock and the
//! repository-id counter, mirroring the filesystem backend so the two reference
//! backends produce identical ids and timestamps for the same call sequence.

use crate::ids::repository_id;
use chrono::{DateTime, Utc};
use std::collections::{BTreeMap, BTreeSet};
use temper_forge_model::{
    CiJob, Comment, ForgeError, ForgeResult, Issue, IssueId, PullRequest, PullRequestId,
    RepoPermission, Repository, RepositoryId, RepositoryPath, User, UserId, WebhookSpec,
};

/// A provisioned user account recorded by the in-memory backend.
///
/// This mirrors [`NewUser`](temper_forge_model::NewUser) but is a plain, cloneable
/// record used by the test-only read-back surface
/// ([`MemoryForge::provisioned_users`](crate::MemoryForge::provisioned_users)).
/// `password` is retained verbatim so tests can assert it; production backends
/// never expose stored passwords.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemUser {
    /// Login/username for the account.
    pub login: String,
    /// Email address for the account.
    pub email: String,
    /// Initial account password recorded at creation.
    pub password: String,
}

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
    labels: BTreeMap<String, Vec<temper_forge_model::Label>>,
    issues: BTreeMap<String, Vec<Issue>>,
    pull_requests: BTreeMap<String, Vec<PullRequest>>,
    issue_comments: BTreeMap<String, Vec<Comment>>,
    pull_request_comments: BTreeMap<String, Vec<Comment>>,
    pull_request_reviews: BTreeMap<String, Vec<temper_forge_model::PullRequestReview>>,
    ci_jobs: BTreeMap<String, Vec<CiJob>>,
    ci_runs: BTreeMap<String, BTreeSet<(Option<String>, String)>>,
    provisioned_users: BTreeMap<String, MemUser>,
    tokens: BTreeMap<String, Vec<String>>,
    token_counters: BTreeMap<String, u64>,
    grants: BTreeMap<String, BTreeMap<String, RepoPermission>>,
    owners_team: BTreeMap<String, BTreeSet<String>>,
    webhooks: BTreeMap<String, Vec<WebhookSpec>>,
    ci_enabled: BTreeSet<String>,
    branches: BTreeMap<String, BTreeSet<String>>,
    files: BTreeMap<(String, String), BTreeMap<String, Vec<u8>>>,
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
            ci_runs: BTreeMap::new(),
            provisioned_users: BTreeMap::new(),
            tokens: BTreeMap::new(),
            token_counters: BTreeMap::new(),
            grants: BTreeMap::new(),
            owners_team: BTreeMap::new(),
            webhooks: BTreeMap::new(),
            ci_enabled: BTreeSet::new(),
            branches: BTreeMap::new(),
            files: BTreeMap::new(),
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

    pub(crate) fn labels_mut(
        &mut self,
        repo_id: &RepositoryId,
    ) -> &mut Vec<temper_forge_model::Label> {
        self.labels.entry(repo_id.as_str().to_string()).or_default()
    }

    pub(crate) fn labels(&self, repo_id: &RepositoryId) -> Vec<temper_forge_model::Label> {
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
    ) -> &mut Vec<temper_forge_model::PullRequestReview> {
        self.pull_request_reviews
            .entry(id.as_str().to_string())
            .or_default()
    }

    pub(crate) fn pull_request_reviews(
        &self,
        id: &PullRequestId,
    ) -> Vec<temper_forge_model::PullRequestReview> {
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

    pub(crate) fn add_ci_run(
        &mut self,
        repo_id: &RepositoryId,
        pull_request_id: Option<&PullRequestId>,
        commit_sha: &str,
    ) {
        self.ci_runs
            .entry(repo_id.as_str().to_string())
            .or_default()
            .insert((
                pull_request_id.map(|id| id.as_str().to_string()),
                commit_sha.to_string(),
            ));
    }

    pub(crate) fn ci_run_matches(
        &self,
        repo_id: &RepositoryId,
        pull_request_id: Option<&PullRequestId>,
        commit_sha: Option<&str>,
    ) -> bool {
        self.ci_runs.get(repo_id.as_str()).is_some_and(|runs| {
            runs.iter().any(|(run_pull_request, run_commit)| {
                pull_request_id.is_none_or(|id| run_pull_request.as_deref() == Some(id.as_str()))
                    && commit_sha.is_none_or(|sha| run_commit == sha)
            })
        })
    }

    /// Finds a CI job by id across every repository.
    pub(crate) fn find_ci_job(&self, id: &temper_forge_model::CiJobId) -> Option<CiJob> {
        self.ci_jobs
            .values()
            .flat_map(|jobs| jobs.iter())
            .find(|job| &job.id == id)
            .cloned()
    }

    // --- Provisioning state (ForgeContent + ForgeAdmin) ---

    /// Idempotently records a provisioned user; an existing login is left as-is.
    pub(crate) fn ensure_user(&mut self, user: MemUser) {
        self.provisioned_users
            .entry(user.login.clone())
            .or_insert(user);
    }

    /// Returns every provisioned user ordered by login.
    pub(crate) fn provisioned_users(&self) -> Vec<MemUser> {
        self.provisioned_users.values().cloned().collect()
    }

    /// Mints the next deterministic token name for `login` and records it.
    pub(crate) fn mint_token(&mut self, login: &str) -> ForgeResult<String> {
        let counter = self
            .token_counters
            .entry(login.to_string())
            .or_insert(0_u64);
        *counter = counter
            .checked_add(1)
            .ok_or_else(|| ForgeError::Backend("token counter overflowed".into()))?;
        let token = format!("mem-token-{login}-{counter}");
        self.tokens
            .entry(login.to_string())
            .or_default()
            .push(token.clone());
        Ok(token)
    }

    /// Returns every token minted for `login` in mint order.
    pub(crate) fn minted_tokens(&self, login: &str) -> Vec<String> {
        self.tokens.get(login).cloned().unwrap_or_default()
    }

    /// Records org Owners-team membership for `owner`/`login`.
    pub(crate) fn add_owner_member(&mut self, owner: &str, login: &str) {
        self.owners_team
            .entry(owner.to_string())
            .or_default()
            .insert(login.to_string());
    }

    /// Ensures an owner exists, creating an empty Owners team if needed.
    pub(crate) fn ensure_owner(&mut self, owner: &str) {
        self.owners_team.entry(owner.to_string()).or_default();
    }

    /// Records a repo-scoped collaborator grant.
    pub(crate) fn grant_repo(&mut self, repo_id: &RepositoryId, login: &str, perm: RepoPermission) {
        self.grants
            .entry(repo_id.as_str().to_string())
            .or_default()
            .insert(login.to_string(), perm);
    }

    /// Returns the repo-scoped collaborator grants for `repo_id`.
    pub(crate) fn grants(&self, repo_id: &RepositoryId) -> BTreeMap<String, RepoPermission> {
        self.grants
            .get(repo_id.as_str())
            .cloned()
            .unwrap_or_default()
    }

    /// Idempotently registers or updates a webhook on `repo_id`, deduplicating on URL.
    pub(crate) fn ensure_webhook(&mut self, repo_id: &RepositoryId, spec: WebhookSpec) {
        let hooks = self
            .webhooks
            .entry(repo_id.as_str().to_string())
            .or_default();
        if let Some(hook) = hooks.iter_mut().find(|hook| hook.url == spec.url) {
            *hook = spec;
            return;
        }
        hooks.push(spec);
    }

    /// Returns the webhooks registered on `repo_id`.
    pub(crate) fn webhooks(&self, repo_id: &RepositoryId) -> Vec<WebhookSpec> {
        self.webhooks
            .get(repo_id.as_str())
            .cloned()
            .unwrap_or_default()
    }

    /// Marks CI as enabled on `repo_id`.
    pub(crate) fn enable_ci(&mut self, repo_id: &RepositoryId) {
        self.ci_enabled.insert(repo_id.as_str().to_string());
    }

    /// Reports whether CI is enabled on `repo_id`.
    pub(crate) fn ci_enabled(&self, repo_id: &RepositoryId) -> bool {
        self.ci_enabled.contains(repo_id.as_str())
    }

    /// Idempotently records a branch on `repo_id`.
    pub(crate) fn create_branch(&mut self, repo_id: &RepositoryId, branch: &str) {
        self.branches
            .entry(repo_id.as_str().to_string())
            .or_default()
            .insert(branch.to_string());
    }

    /// Reports whether `branch` is recorded on `repo_id`.
    pub(crate) fn branch_exists(&self, repo_id: &RepositoryId, branch: &str) -> bool {
        self.branches
            .get(repo_id.as_str())
            .is_some_and(|branches| branches.contains(branch))
    }

    /// Upserts a committed file at `(repo_id, branch, path)`.
    pub(crate) fn commit_file(
        &mut self,
        repo_id: &RepositoryId,
        branch: &str,
        path: &str,
        contents: Vec<u8>,
    ) {
        self.files
            .entry((repo_id.as_str().to_string(), branch.to_string()))
            .or_default()
            .insert(path.to_string(), contents);
        self.branches
            .entry(repo_id.as_str().to_string())
            .or_default()
            .insert(branch.to_string());
    }

    /// Returns the committed file contents at `(repo_id, branch, path)`.
    pub(crate) fn committed_file(
        &self,
        repo_id: &RepositoryId,
        branch: &str,
        path: &str,
    ) -> Option<Vec<u8>> {
        self.files
            .get(&(repo_id.as_str().to_string(), branch.to_string()))
            .and_then(|files| files.get(path))
            .cloned()
    }
}

fn timestamp_from_tick(tick: u64) -> ForgeResult<DateTime<Utc>> {
    let seconds = i64::try_from(tick)
        .map_err(|_| ForgeError::Backend("in-memory logical clock is too large".into()))?;
    DateTime::<Utc>::from_timestamp(seconds, 0)
        .ok_or_else(|| ForgeError::Backend("in-memory logical clock is out of range".into()))
}
