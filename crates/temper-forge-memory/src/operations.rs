//! [`Forge`] implementation for [`MemoryForge`](crate::MemoryForge).
//!
//! Every method takes the single interior mutex for the duration of the call,
//! checks the [fault hook](crate::fault), then reads or mutates the in-memory
//! [`State`](crate::state::State). The behaviour mirrors the filesystem backend
//! contract documented in `docs/reference/in-memory-backend.md`.

use crate::dependencies::{
    add_issue_dependency, add_pull_request_dependency, remove_issue_dependency,
    remove_pull_request_dependency,
};
use crate::fault::FaultOp;
use crate::ids::{
    issue_comment_id, issue_id, label_id, merge_commit_sha, pull_request_comment_id,
    pull_request_id,
};
use crate::lists::{
    apply_assignee_update, apply_label_update, ci_job_matches_query, issue_matches_query,
    normalize_string_set, normalize_user_set, pull_request_matches_query, sort_ci_jobs,
    sort_comments, sort_issues, sort_issues_by_number, sort_labels, sort_pull_requests,
    sort_pull_requests_by_number, sort_repositories, update_issue_state, update_pull_request_state,
};
use crate::util::{
    check_expected_version, next_comment_number, next_item_number, validate_create_repository,
    validate_upsert_label,
};
use async_trait::async_trait;
use temper_forge::{
    ChangeKind, CiJob, CiJobId, CiJobQuery, Comment, CreateComment, CreateIssue, CreatePullRequest,
    CreatePullRequestReview, CreateRepository, Forge, ForgeError, ForgeResult, Issue, IssueId,
    IssueQuery, IssueState, ItemNumber, Label, MergePullRequest, MergeRecord, PullRequest,
    PullRequestId, PullRequestQuery, PullRequestReview, PullRequestState, Repository, RepositoryId,
    RepositoryPath, RepositoryQuery, RequestReviewers, UpdateIssue, UpdatePullRequest, UpsertLabel,
    User, UserId, Version,
};

use crate::MemoryForge;
use crate::reviews::{list_reviews, request_reviewers, submit_review};

#[async_trait]
impl Forge for MemoryForge {
    async fn current_user(&self) -> ForgeResult<User> {
        let inner = self.lock();
        Ok(self.effective_user(&inner))
    }

    async fn get_user(&self, id: &UserId) -> ForgeResult<Option<User>> {
        let inner = self.lock();
        let user = self.effective_user(&inner);
        Ok((&user.id == id).then_some(user))
    }

    async fn list_repositories(&self, query: RepositoryQuery) -> ForgeResult<Vec<Repository>> {
        let mut repositories = self.lock().state.repositories();
        sort_repositories(&mut repositories, &query);
        Ok(repositories)
    }

    async fn create_repository(&self, input: CreateRepository) -> ForgeResult<Repository> {
        validate_create_repository(&input)?;

        let mut inner = self.lock();
        let path = RepositoryPath::new(input.owner.clone(), input.name.clone());
        if inner.state.find_repository_by_path(&path).is_some() {
            return Err(ForgeError::AlreadyExists(format!(
                "repository {}/{}",
                input.owner, input.name
            )));
        }

        let now = inner.state.next_timestamp()?;
        let id = inner.state.allocate_repository_id()?;
        let repository = Repository {
            id,
            owner: input.owner,
            name: input.name,
            default_branch: input.default_branch,
            description: input.description,
            created_at: now,
            updated_at: now,
        };
        inner.state.insert_repository(repository.clone());
        let path = RepositoryPath::new(repository.owner.clone(), repository.name.clone());
        inner.publish_path_hint(path, ChangeKind::Unknown);
        Ok(repository)
    }

    async fn get_repository(&self, id: &RepositoryId) -> ForgeResult<Option<Repository>> {
        Ok(self.lock().state.find_repository_by_id(id))
    }

    async fn get_repository_by_path(
        &self,
        path: &RepositoryPath,
    ) -> ForgeResult<Option<Repository>> {
        Ok(self.lock().state.find_repository_by_path(path))
    }

    async fn list_labels(&self, repo_id: &RepositoryId) -> ForgeResult<Vec<Label>> {
        let inner = self.lock();
        inner.state.require_repository(repo_id)?;
        let mut labels = inner.state.labels(repo_id);
        sort_labels(&mut labels);
        Ok(labels)
    }

    async fn upsert_label(&self, repo_id: &RepositoryId, input: UpsertLabel) -> ForgeResult<Label> {
        validate_upsert_label(&input)?;

        let mut inner = self.lock();
        inner.state.require_repository(repo_id)?;
        let id = label_id(repo_id, &input.name);
        let labels = inner.state.labels_mut(repo_id);
        let label = if let Some(existing) = labels.iter_mut().find(|label| label.name == input.name)
        {
            existing.color = input.color;
            existing.description = input.description;
            existing.clone()
        } else {
            let label = Label {
                id,
                repo_id: repo_id.clone(),
                name: input.name,
                color: input.color,
                description: input.description,
            };
            labels.push(label.clone());
            label
        };
        sort_labels(labels);
        inner.publish_repo_hint(repo_id, ChangeKind::Label);
        Ok(label)
    }

    async fn list_issues(
        &self,
        repo_id: &RepositoryId,
        query: IssueQuery,
    ) -> ForgeResult<Vec<Issue>> {
        let mut inner = self.lock();
        inner.faults.take(FaultOp::ListIssues)?;
        inner.state.require_repository(repo_id)?;
        let mut issues = inner
            .state
            .issues(repo_id)
            .into_iter()
            .filter(|issue| issue_matches_query(issue, &query))
            .collect::<Vec<_>>();
        if !query.details.dependencies {
            for issue in &mut issues {
                issue.dependencies.clear();
            }
        }
        sort_issues(&mut issues, &query);
        Ok(issues)
    }

    async fn create_issue(&self, repo_id: &RepositoryId, input: CreateIssue) -> ForgeResult<Issue> {
        let mut inner = self.lock();
        inner.faults.take(FaultOp::CreateIssue)?;
        inner.state.require_repository(repo_id)?;
        let now = inner.state.next_timestamp()?;
        let author_id = self.effective_user(&inner).id;
        let issues = inner.state.issues_mut(repo_id);
        let number = next_item_number(issues.iter().map(|issue| issue.number))?;
        let issue = Issue {
            id: issue_id(repo_id, number),
            repo_id: repo_id.clone(),
            number,
            title: input.title,
            body: input.body,
            state: IssueState::Open,
            author_id,
            labels: normalize_string_set(input.labels),
            assignees: normalize_user_set(input.assignees),
            dependencies: Vec::new(),
            version: Version::INITIAL,
            created_at: now,
            updated_at: now,
            closed_at: None,
        };
        issues.push(issue.clone());
        sort_issues_by_number(issues);
        inner.publish_item_hint(repo_id, issue.number, ChangeKind::Issue);
        Ok(issue)
    }

    async fn get_issue(&self, id: &IssueId) -> ForgeResult<Option<Issue>> {
        Ok(self.lock().state.find_issue(id).map(|(_, issue)| issue))
    }

    async fn get_issue_by_number(
        &self,
        repo_id: &RepositoryId,
        number: ItemNumber,
    ) -> ForgeResult<Option<Issue>> {
        let mut inner = self.lock();
        inner.faults.take(FaultOp::GetIssueByNumber)?;
        if !inner.state.repository_exists(repo_id) {
            return Ok(None);
        }
        Ok(inner
            .state
            .issues(repo_id)
            .into_iter()
            .find(|issue| issue.number == number))
    }

    async fn update_issue(&self, id: &IssueId, input: UpdateIssue) -> ForgeResult<Issue> {
        let mut inner = self.lock();
        inner.faults.take(FaultOp::UpdateIssue)?;
        let (repo_id, existing) = inner
            .state
            .find_issue(id)
            .ok_or_else(|| ForgeError::NotFound(format!("issue {id}")))?;
        check_expected_version("issue", id, input.expected_version, existing.version)?;
        let now = inner.state.next_timestamp()?;
        let issues = inner.state.issues_mut(&repo_id);
        let issue = issues
            .iter_mut()
            .find(|issue| &issue.id == id)
            .ok_or_else(|| ForgeError::NotFound(format!("issue {id}")))?;

        if let Some(title) = input.title {
            issue.title = title;
        }
        if let Some(body) = input.body {
            issue.body = body;
        }
        if let Some(state) = input.state {
            update_issue_state(issue, state, now);
        }
        apply_label_update(
            &mut issue.labels,
            input.set_labels,
            input.remove_labels,
            input.add_labels,
        );
        apply_assignee_update(
            &mut issue.assignees,
            input.remove_assignees,
            input.add_assignees,
        );
        issue.version = issue.version.next();
        issue.updated_at = now;
        let updated = issue.clone();
        sort_issues_by_number(issues);
        inner.publish_item_hint(&repo_id, updated.number, ChangeKind::Issue);
        Ok(updated)
    }

    async fn add_issue_dependency(&self, id: &IssueId, target: ItemNumber) -> ForgeResult<Issue> {
        let mut inner = self.lock();
        let (issue, changed) = add_issue_dependency(&mut inner, id, target)?;
        if changed {
            inner.publish_item_hint(&issue.repo_id, issue.number, ChangeKind::Issue);
        }
        Ok(issue)
    }

    async fn remove_issue_dependency(
        &self,
        id: &IssueId,
        target: ItemNumber,
    ) -> ForgeResult<Issue> {
        let mut inner = self.lock();
        let (issue, changed) = remove_issue_dependency(&mut inner, id, target)?;
        if changed {
            inner.publish_item_hint(&issue.repo_id, issue.number, ChangeKind::Issue);
        }
        Ok(issue)
    }

    async fn list_issue_comments(&self, id: &IssueId) -> ForgeResult<Vec<Comment>> {
        let inner = self.lock();
        inner
            .state
            .find_issue(id)
            .ok_or_else(|| ForgeError::NotFound(format!("issue {id}")))?;
        let mut comments = inner.state.issue_comments(id);
        sort_comments(&mut comments);
        Ok(comments)
    }

    async fn add_issue_comment(&self, id: &IssueId, input: CreateComment) -> ForgeResult<Comment> {
        let mut inner = self.lock();
        inner
            .state
            .find_issue(id)
            .ok_or_else(|| ForgeError::NotFound(format!("issue {id}")))?;
        let now = inner.state.next_timestamp()?;
        let author_id = self.effective_user(&inner).id;
        let comments = inner.state.issue_comments_mut(id);
        let number = next_comment_number(comments.len())?;
        let comment = Comment {
            id: issue_comment_id(id, number),
            author_id,
            body: input.body,
            created_at: now,
            updated_at: now,
        };
        comments.push(comment.clone());
        sort_comments(comments);
        if let Some((repo_id, issue)) = inner.state.find_issue(id) {
            inner.publish_item_hint(&repo_id, issue.number, ChangeKind::Comment);
        }
        Ok(comment)
    }

    async fn list_pull_requests(
        &self,
        repo_id: &RepositoryId,
        query: PullRequestQuery,
    ) -> ForgeResult<Vec<PullRequest>> {
        let mut inner = self.lock();
        inner.faults.take(FaultOp::ListPullRequests)?;
        inner.state.require_repository(repo_id)?;
        let mut pull_requests = inner
            .state
            .pull_requests(repo_id)
            .into_iter()
            .filter(|pull_request| pull_request_matches_query(pull_request, &query))
            .collect::<Vec<_>>();
        if !query.details.dependencies {
            for pull_request in &mut pull_requests {
                pull_request.dependencies.clear();
            }
        }
        sort_pull_requests(&mut pull_requests, &query);
        Ok(pull_requests)
    }

    async fn create_pull_request(
        &self,
        repo_id: &RepositoryId,
        input: CreatePullRequest,
    ) -> ForgeResult<PullRequest> {
        let mut inner = self.lock();
        inner.faults.take(FaultOp::CreatePullRequest)?;
        inner.state.require_repository(repo_id)?;
        let now = inner.state.next_timestamp()?;
        let author_id = self.effective_user(&inner).id;
        let pull_requests = inner.state.pull_requests_mut(repo_id);
        let number = next_item_number(pull_requests.iter().map(|pr| pr.number))?;
        let pull_request = PullRequest {
            id: pull_request_id(repo_id, number),
            repo_id: repo_id.clone(),
            number,
            title: input.title,
            body: input.body,
            state: PullRequestState::Open,
            author_id,
            source: input.source,
            target: input.target,
            head_sha: None,
            base_sha: None,
            labels: normalize_string_set(input.labels),
            assignees: normalize_user_set(input.assignees),
            requested_reviewers: Vec::new(),
            dependencies: Vec::new(),
            merge: None,
            version: Version::INITIAL,
            created_at: now,
            updated_at: now,
            closed_at: None,
        };
        pull_requests.push(pull_request.clone());
        sort_pull_requests_by_number(pull_requests);
        inner.publish_item_hint(repo_id, pull_request.number, ChangeKind::PullRequest);
        Ok(pull_request)
    }

    async fn get_pull_request(&self, id: &PullRequestId) -> ForgeResult<Option<PullRequest>> {
        Ok(self
            .lock()
            .state
            .find_pull_request(id)
            .map(|(_, pull_request)| pull_request))
    }

    async fn get_pull_request_by_number(
        &self,
        repo_id: &RepositoryId,
        number: ItemNumber,
    ) -> ForgeResult<Option<PullRequest>> {
        let mut inner = self.lock();
        inner.faults.take(FaultOp::GetPullRequestByNumber)?;
        if !inner.state.repository_exists(repo_id) {
            return Ok(None);
        }
        Ok(inner
            .state
            .pull_requests(repo_id)
            .into_iter()
            .find(|pull_request| pull_request.number == number))
    }

    async fn update_pull_request(
        &self,
        id: &PullRequestId,
        input: UpdatePullRequest,
    ) -> ForgeResult<PullRequest> {
        let mut inner = self.lock();
        inner.faults.take(FaultOp::UpdatePullRequest)?;
        let (repo_id, existing) = inner
            .state
            .find_pull_request(id)
            .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;
        check_expected_version("pull request", id, input.expected_version, existing.version)?;
        let now = inner.state.next_timestamp()?;
        let pull_requests = inner.state.pull_requests_mut(&repo_id);
        let pull_request = pull_requests
            .iter_mut()
            .find(|pull_request| &pull_request.id == id)
            .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;

        if let Some(title) = input.title {
            pull_request.title = title;
        }
        if let Some(body) = input.body {
            pull_request.body = body;
        }
        if let Some(state) = input.state {
            update_pull_request_state(pull_request, state, now)?;
        }
        apply_label_update(
            &mut pull_request.labels,
            input.set_labels,
            input.remove_labels,
            input.add_labels,
        );
        apply_assignee_update(
            &mut pull_request.assignees,
            input.remove_assignees,
            input.add_assignees,
        );
        pull_request.version = pull_request.version.next();
        pull_request.updated_at = now;
        let updated = pull_request.clone();
        sort_pull_requests_by_number(pull_requests);
        inner.publish_item_hint(&repo_id, updated.number, ChangeKind::PullRequest);
        Ok(updated)
    }

    async fn add_pull_request_dependency(
        &self,
        id: &PullRequestId,
        target: ItemNumber,
    ) -> ForgeResult<PullRequest> {
        let mut inner = self.lock();
        let (pull_request, changed) = add_pull_request_dependency(&mut inner, id, target)?;
        if changed {
            inner.publish_pull_request_hint(&pull_request, ChangeKind::PullRequest);
        }
        Ok(pull_request)
    }

    async fn remove_pull_request_dependency(
        &self,
        id: &PullRequestId,
        target: ItemNumber,
    ) -> ForgeResult<PullRequest> {
        let mut inner = self.lock();
        let (pull_request, changed) = remove_pull_request_dependency(&mut inner, id, target)?;
        if changed {
            inner.publish_pull_request_hint(&pull_request, ChangeKind::PullRequest);
        }
        Ok(pull_request)
    }

    async fn request_pull_request_reviewers(
        &self,
        id: &PullRequestId,
        input: RequestReviewers,
    ) -> ForgeResult<PullRequest> {
        let (pull_request, changed) = request_reviewers(self, id, input)?;
        if changed {
            self.lock()
                .publish_pull_request_hint(&pull_request, ChangeKind::Review);
        }
        Ok(pull_request)
    }

    async fn list_pull_request_reviews(
        &self,
        id: &PullRequestId,
    ) -> ForgeResult<Vec<PullRequestReview>> {
        list_reviews(self, id)
    }

    async fn submit_pull_request_review(
        &self,
        id: &PullRequestId,
        input: CreatePullRequestReview,
    ) -> ForgeResult<PullRequestReview> {
        let (review, repo_id, number) = submit_review(self, id, input)?;
        self.lock()
            .publish_item_hint(&repo_id, number, ChangeKind::Review);
        Ok(review)
    }

    async fn list_pull_request_comments(&self, id: &PullRequestId) -> ForgeResult<Vec<Comment>> {
        let inner = self.lock();
        inner
            .state
            .find_pull_request(id)
            .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;
        let mut comments = inner.state.pull_request_comments(id);
        sort_comments(&mut comments);
        Ok(comments)
    }

    async fn add_pull_request_comment(
        &self,
        id: &PullRequestId,
        input: CreateComment,
    ) -> ForgeResult<Comment> {
        let mut inner = self.lock();
        inner
            .state
            .find_pull_request(id)
            .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;
        let now = inner.state.next_timestamp()?;
        let author_id = self.effective_user(&inner).id;
        let comments = inner.state.pull_request_comments_mut(id);
        let number = next_comment_number(comments.len())?;
        let comment = Comment {
            id: pull_request_comment_id(id, number),
            author_id,
            body: input.body,
            created_at: now,
            updated_at: now,
        };
        comments.push(comment.clone());
        sort_comments(comments);
        if let Some((repo_id, pull_request)) = inner.state.find_pull_request(id) {
            inner.publish_item_hint(&repo_id, pull_request.number, ChangeKind::Comment);
        }
        Ok(comment)
    }

    async fn merge_pull_request(
        &self,
        id: &PullRequestId,
        input: MergePullRequest,
    ) -> ForgeResult<MergeRecord> {
        let mut inner = self.lock();
        inner.faults.take(FaultOp::MergePullRequest)?;
        let (repo_id, _) = inner
            .state
            .find_pull_request(id)
            .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;
        let now = inner.state.next_timestamp()?;
        let clock_tick = inner.state.clock_tick();
        let merged_by = self.effective_user(&inner).id;
        let pull_requests = inner.state.pull_requests_mut(&repo_id);
        let pull_request = pull_requests
            .iter_mut()
            .find(|pull_request| &pull_request.id == id)
            .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;

        match pull_request.state {
            PullRequestState::Open => {}
            PullRequestState::Closed => {
                return Err(ForgeError::Conflict(format!("pull request {id} is closed")));
            }
            PullRequestState::Merged => {
                return Err(ForgeError::Conflict(format!("pull request {id} is merged")));
            }
        }

        let merge = MergeRecord {
            method: input.method,
            commit_sha: merge_commit_sha(clock_tick),
            merged_by,
            merged_at: now,
        };
        pull_request.state = PullRequestState::Merged;
        pull_request.merge = Some(merge.clone());
        pull_request.version = pull_request.version.next();
        pull_request.updated_at = now;
        let number = pull_request.number;
        pull_request.closed_at = Some(now);
        sort_pull_requests_by_number(pull_requests);
        inner.publish_item_hint(&repo_id, number, ChangeKind::PullRequest);
        Ok(merge)
    }

    async fn list_ci_jobs(
        &self,
        repo_id: &RepositoryId,
        query: CiJobQuery,
    ) -> ForgeResult<Vec<CiJob>> {
        let inner = self.lock();
        inner.state.require_repository(repo_id)?;
        let mut ci_jobs = inner
            .state
            .ci_jobs(repo_id)
            .into_iter()
            .filter(|ci_job| ci_job_matches_query(ci_job, &query))
            .collect::<Vec<_>>();
        sort_ci_jobs(&mut ci_jobs, &query);
        Ok(ci_jobs)
    }

    async fn get_ci_job(&self, id: &CiJobId) -> ForgeResult<Option<CiJob>> {
        Ok(self.lock().state.find_ci_job(id))
    }
}
