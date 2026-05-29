use crate::dependencies::{
    add_issue_dependency, add_pull_request_dependency, remove_issue_dependency,
    remove_pull_request_dependency,
};
use crate::lists::{
    apply_assignee_update, apply_label_update, ci_job_matches_query, issue_matches_query,
    next_issue_comment_number, next_issue_number, next_pull_request_comment_number,
    next_pull_request_number, normalize_string_set, normalize_user_set, pull_request_matches_query,
    sort_ci_jobs, sort_comments, sort_issues, sort_issues_by_number, sort_labels,
    sort_pull_requests, sort_pull_requests_by_number, sort_repositories, update_issue_state,
    update_pull_request_state,
};
use crate::metadata::next_timestamp;
use crate::record_ids::{
    issue_comment_id, issue_id, label_id, merge_commit_sha, pull_request_comment_id,
    pull_request_id,
};
use crate::validation::{validate_create_repository, validate_upsert_label};
use crate::FilesystemForge;
use async_trait::async_trait;
use harness_forge::{
    CiJob, CiJobId, CiJobQuery, Comment, CreateComment, CreateIssue, CreatePullRequest,
    CreateRepository, Forge, ForgeError, ForgeResult, Issue, IssueId, IssueQuery, IssueState,
    ItemNumber, Label, MergePullRequest, MergeRecord, PullRequest, PullRequestId, PullRequestQuery,
    PullRequestState, Repository, RepositoryId, RepositoryPath, RepositoryQuery, UpdateIssue,
    UpdatePullRequest, UpsertLabel, User, UserId, Version,
};

/// Enforces an optimistic-concurrency precondition.
///
/// Returns [`ForgeError::Conflict`] when `expected` is `Some` and does not equal
/// the stored `actual` version; a `None` precondition always passes (an
/// unconditional update). Checked before the logical clock advances or any file
/// is written, so a rejected conditional update leaves the store untouched.
fn check_expected_version(
    kind: &str,
    id: &impl std::fmt::Display,
    expected: Option<Version>,
    actual: Version,
) -> ForgeResult<()> {
    match expected {
        Some(expected) if expected != actual => Err(ForgeError::Conflict(format!(
            "{kind} {id} expected version {expected} but found {actual}"
        ))),
        _ => Ok(()),
    }
}

#[async_trait]
impl Forge for FilesystemForge {
    async fn current_user(&self) -> ForgeResult<User> {
        Ok(self.read_metadata()?.current_user)
    }

    async fn get_user(&self, id: &UserId) -> ForgeResult<Option<User>> {
        let user = self.current_user().await?;
        Ok((&user.id == id).then_some(user))
    }

    async fn list_repositories(&self, query: RepositoryQuery) -> ForgeResult<Vec<Repository>> {
        let mut repositories = self.read_repositories()?;
        sort_repositories(&mut repositories, query);
        Ok(repositories)
    }

    async fn create_repository(&self, input: CreateRepository) -> ForgeResult<Repository> {
        validate_create_repository(&input)?;

        let path = RepositoryPath::new(input.owner.clone(), input.name.clone());
        if self.find_repository_by_path(&path)?.is_some() {
            return Err(ForgeError::AlreadyExists(format!(
                "repository {}/{}",
                input.owner, input.name
            )));
        }

        let mut metadata = self.read_metadata()?;
        let now = next_timestamp(&mut metadata)?;
        let repository = Repository {
            id: self.next_repository_id(&mut metadata)?,
            owner: input.owner,
            name: input.name,
            default_branch: input.default_branch,
            description: input.description,
            created_at: now,
            updated_at: now,
        };

        let repository_path = self.repository_file(&repository.id).ok_or_else(|| {
            ForgeError::Backend(format!("generated invalid repository id {}", repository.id))
        })?;
        self.write_json(&repository_path, &repository)?;
        self.write_metadata(&metadata)?;

        Ok(repository)
    }

    async fn get_repository(&self, id: &RepositoryId) -> ForgeResult<Option<Repository>> {
        self.find_repository_by_id(id)
    }

    async fn get_repository_by_path(
        &self,
        path: &RepositoryPath,
    ) -> ForgeResult<Option<Repository>> {
        self.find_repository_by_path(path)
    }

    async fn list_labels(&self, repo_id: &RepositoryId) -> ForgeResult<Vec<Label>> {
        self.require_repository(repo_id)?;
        self.read_labels_for_existing_repository(repo_id)
    }

    async fn upsert_label(&self, repo_id: &RepositoryId, input: UpsertLabel) -> ForgeResult<Label> {
        self.require_repository(repo_id)?;
        validate_upsert_label(&input)?;

        let mut labels = self.read_labels_for_existing_repository(repo_id)?;
        let label = if let Some(existing) = labels.iter_mut().find(|label| label.name == input.name)
        {
            existing.color = input.color;
            existing.description = input.description;
            existing.clone()
        } else {
            let label = Label {
                id: label_id(repo_id, &input.name),
                repo_id: repo_id.clone(),
                name: input.name,
                color: input.color,
                description: input.description,
            };
            labels.push(label.clone());
            label
        };

        sort_labels(&mut labels);
        self.write_labels(repo_id, &labels)?;

        Ok(label)
    }

    async fn list_issues(
        &self,
        repo_id: &RepositoryId,
        query: IssueQuery,
    ) -> ForgeResult<Vec<Issue>> {
        self.require_repository(repo_id)?;

        let mut issues = self
            .read_issues_for_existing_repository(repo_id)?
            .into_iter()
            .filter(|issue| issue_matches_query(issue, &query))
            .collect::<Vec<_>>();
        sort_issues(&mut issues, &query);
        Ok(issues)
    }

    async fn create_issue(&self, repo_id: &RepositoryId, input: CreateIssue) -> ForgeResult<Issue> {
        self.require_repository(repo_id)?;

        let mut metadata = self.read_metadata()?;
        let mut issues = self.read_issues_for_existing_repository(repo_id)?;
        let number = next_issue_number(repo_id, &issues)?;
        let now = next_timestamp(&mut metadata)?;
        let issue = Issue {
            id: issue_id(repo_id, number),
            repo_id: repo_id.clone(),
            number,
            title: input.title,
            body: input.body,
            state: IssueState::Open,
            author_id: metadata.current_user.id.clone(),
            labels: normalize_string_set(input.labels),
            assignees: normalize_user_set(input.assignees),
            dependencies: Vec::new(),
            version: Version::INITIAL,
            created_at: now,
            updated_at: now,
            closed_at: None,
        };

        issues.push(issue.clone());
        sort_issues_by_number(&mut issues);
        self.write_issues(repo_id, &issues)?;
        self.write_metadata(&metadata)?;

        Ok(issue)
    }

    async fn get_issue(&self, id: &IssueId) -> ForgeResult<Option<Issue>> {
        self.find_issue_by_id(id)
    }

    async fn get_issue_by_number(
        &self,
        repo_id: &RepositoryId,
        number: ItemNumber,
    ) -> ForgeResult<Option<Issue>> {
        if self.find_repository_by_id(repo_id)?.is_none() {
            return Ok(None);
        }

        Ok(self
            .read_issues_for_existing_repository(repo_id)?
            .into_iter()
            .find(|issue| issue.number == number))
    }

    async fn update_issue(&self, id: &IssueId, input: UpdateIssue) -> ForgeResult<Issue> {
        let repo_id = self
            .find_issue_repository_by_id(id)?
            .ok_or_else(|| ForgeError::NotFound(format!("issue {id}")))?;

        let mut issues = self.read_issues_for_existing_repository(&repo_id)?;
        let issue = issues
            .iter_mut()
            .find(|issue| &issue.id == id)
            .ok_or_else(|| ForgeError::NotFound(format!("issue {id}")))?;
        check_expected_version("issue", id, input.expected_version, issue.version)?;

        let mut metadata = self.read_metadata()?;
        let now = next_timestamp(&mut metadata)?;

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
        sort_issues_by_number(&mut issues);
        self.write_issues(&repo_id, &issues)?;
        self.write_metadata(&metadata)?;

        Ok(updated)
    }

    async fn add_issue_dependency(&self, id: &IssueId, target: ItemNumber) -> ForgeResult<Issue> {
        add_issue_dependency(self, id, target)
    }

    async fn remove_issue_dependency(
        &self,
        id: &IssueId,
        target: ItemNumber,
    ) -> ForgeResult<Issue> {
        remove_issue_dependency(self, id, target)
    }

    async fn list_issue_comments(&self, id: &IssueId) -> ForgeResult<Vec<Comment>> {
        let repo_id = self
            .find_issue_repository_by_id(id)?
            .ok_or_else(|| ForgeError::NotFound(format!("issue {id}")))?;

        self.read_issue_comments_for_existing_issue(&repo_id, id)
    }

    async fn add_issue_comment(&self, id: &IssueId, input: CreateComment) -> ForgeResult<Comment> {
        let repo_id = self
            .find_issue_repository_by_id(id)?
            .ok_or_else(|| ForgeError::NotFound(format!("issue {id}")))?;

        let mut comments = self.read_issue_comments_for_existing_issue(&repo_id, id)?;
        let comment_number = next_issue_comment_number(id, &comments)?;
        let mut metadata = self.read_metadata()?;
        let now = next_timestamp(&mut metadata)?;
        let comment = Comment {
            id: issue_comment_id(id, comment_number),
            author_id: metadata.current_user.id.clone(),
            body: input.body,
            created_at: now,
            updated_at: now,
        };

        comments.push(comment.clone());
        sort_comments(&mut comments);
        self.write_issue_comments(&repo_id, id, &comments)?;
        self.write_metadata(&metadata)?;

        Ok(comment)
    }

    async fn list_pull_requests(
        &self,
        repo_id: &RepositoryId,
        query: PullRequestQuery,
    ) -> ForgeResult<Vec<PullRequest>> {
        self.require_repository(repo_id)?;

        let mut pull_requests = self
            .read_pull_requests_for_existing_repository(repo_id)?
            .into_iter()
            .filter(|pull_request| pull_request_matches_query(pull_request, &query))
            .collect::<Vec<_>>();
        sort_pull_requests(&mut pull_requests, &query);
        Ok(pull_requests)
    }

    async fn create_pull_request(
        &self,
        repo_id: &RepositoryId,
        input: CreatePullRequest,
    ) -> ForgeResult<PullRequest> {
        self.require_repository(repo_id)?;

        let mut metadata = self.read_metadata()?;
        let mut pull_requests = self.read_pull_requests_for_existing_repository(repo_id)?;
        let number = next_pull_request_number(repo_id, &pull_requests)?;
        let now = next_timestamp(&mut metadata)?;
        let pull_request = PullRequest {
            id: pull_request_id(repo_id, number),
            repo_id: repo_id.clone(),
            number,
            title: input.title,
            body: input.body,
            state: PullRequestState::Open,
            author_id: metadata.current_user.id.clone(),
            source: input.source,
            target: input.target,
            head_sha: None,
            base_sha: None,
            labels: normalize_string_set(input.labels),
            assignees: normalize_user_set(input.assignees),
            dependencies: Vec::new(),
            merge: None,
            version: Version::INITIAL,
            created_at: now,
            updated_at: now,
            closed_at: None,
        };

        pull_requests.push(pull_request.clone());
        sort_pull_requests_by_number(&mut pull_requests);
        self.write_pull_requests(repo_id, &pull_requests)?;
        self.write_metadata(&metadata)?;

        Ok(pull_request)
    }

    async fn get_pull_request(&self, id: &PullRequestId) -> ForgeResult<Option<PullRequest>> {
        self.find_pull_request_by_id(id)
    }

    async fn get_pull_request_by_number(
        &self,
        repo_id: &RepositoryId,
        number: ItemNumber,
    ) -> ForgeResult<Option<PullRequest>> {
        if self.find_repository_by_id(repo_id)?.is_none() {
            return Ok(None);
        }

        Ok(self
            .read_pull_requests_for_existing_repository(repo_id)?
            .into_iter()
            .find(|pull_request| pull_request.number == number))
    }

    async fn update_pull_request(
        &self,
        id: &PullRequestId,
        input: UpdatePullRequest,
    ) -> ForgeResult<PullRequest> {
        let repo_id = self
            .find_pull_request_repository_by_id(id)?
            .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;

        let mut pull_requests = self.read_pull_requests_for_existing_repository(&repo_id)?;
        let pull_request = pull_requests
            .iter_mut()
            .find(|pull_request| &pull_request.id == id)
            .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;
        check_expected_version(
            "pull request",
            id,
            input.expected_version,
            pull_request.version,
        )?;

        let mut metadata = self.read_metadata()?;
        let now = next_timestamp(&mut metadata)?;

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
        sort_pull_requests_by_number(&mut pull_requests);
        self.write_pull_requests(&repo_id, &pull_requests)?;
        self.write_metadata(&metadata)?;

        Ok(updated)
    }

    async fn add_pull_request_dependency(
        &self,
        id: &PullRequestId,
        target: ItemNumber,
    ) -> ForgeResult<PullRequest> {
        add_pull_request_dependency(self, id, target)
    }

    async fn remove_pull_request_dependency(
        &self,
        id: &PullRequestId,
        target: ItemNumber,
    ) -> ForgeResult<PullRequest> {
        remove_pull_request_dependency(self, id, target)
    }

    async fn list_pull_request_comments(&self, id: &PullRequestId) -> ForgeResult<Vec<Comment>> {
        let repo_id = self
            .find_pull_request_repository_by_id(id)?
            .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;

        self.read_pull_request_comments_for_existing_pull_request(&repo_id, id)
    }

    async fn add_pull_request_comment(
        &self,
        id: &PullRequestId,
        input: CreateComment,
    ) -> ForgeResult<Comment> {
        let repo_id = self
            .find_pull_request_repository_by_id(id)?
            .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;

        let mut comments =
            self.read_pull_request_comments_for_existing_pull_request(&repo_id, id)?;
        let comment_number = next_pull_request_comment_number(id, &comments)?;
        let mut metadata = self.read_metadata()?;
        let now = next_timestamp(&mut metadata)?;
        let comment = Comment {
            id: pull_request_comment_id(id, comment_number),
            author_id: metadata.current_user.id.clone(),
            body: input.body,
            created_at: now,
            updated_at: now,
        };

        comments.push(comment.clone());
        sort_comments(&mut comments);
        self.write_pull_request_comments(&repo_id, id, &comments)?;
        self.write_metadata(&metadata)?;

        Ok(comment)
    }

    async fn merge_pull_request(
        &self,
        id: &PullRequestId,
        input: MergePullRequest,
    ) -> ForgeResult<MergeRecord> {
        let repo_id = self
            .find_pull_request_repository_by_id(id)?
            .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;

        let mut pull_requests = self.read_pull_requests_for_existing_repository(&repo_id)?;
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

        let mut metadata = self.read_metadata()?;
        let now = next_timestamp(&mut metadata)?;
        let merge = MergeRecord {
            method: input.method,
            commit_sha: merge_commit_sha(metadata.clock_tick),
            merged_by: metadata.current_user.id.clone(),
            merged_at: now,
        };

        pull_request.state = PullRequestState::Merged;
        pull_request.merge = Some(merge.clone());
        pull_request.version = pull_request.version.next();
        pull_request.updated_at = now;
        pull_request.closed_at = Some(now);

        sort_pull_requests_by_number(&mut pull_requests);
        self.write_pull_requests(&repo_id, &pull_requests)?;
        self.write_metadata(&metadata)?;

        Ok(merge)
    }

    async fn list_ci_jobs(
        &self,
        repo_id: &RepositoryId,
        query: CiJobQuery,
    ) -> ForgeResult<Vec<CiJob>> {
        self.require_repository(repo_id)?;

        let mut ci_jobs = self
            .read_ci_jobs_for_existing_repository(repo_id)?
            .into_iter()
            .filter(|ci_job| ci_job_matches_query(ci_job, &query))
            .collect::<Vec<_>>();
        sort_ci_jobs(&mut ci_jobs, &query);
        Ok(ci_jobs)
    }

    async fn get_ci_job(&self, id: &CiJobId) -> ForgeResult<Option<CiJob>> {
        self.find_ci_job_by_id(id)
    }
}
