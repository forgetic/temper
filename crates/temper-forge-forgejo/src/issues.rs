//! Issue operations: list/get/create/update plus issue comments.
//!
//! These are inherent methods on [`ForgejoForge`] mirroring the
//! [`temper_forge::Forge`] issue surface; the trait implementation is assembled
//! once every phase's methods exist. Forgejo serves both issues and pull
//! requests through the issue endpoints, so the read paths exclude PR-as-issue
//! rows (a pull request is never returned as an [`Issue`]). Label and assignee
//! changes reuse the shared item helpers in [`crate::items`] so issues and pull
//! requests share one sequencing implementation. The conditional-update path
//! mirrors [`crate::pulls::ForgejoForge::update_pull_request`]. See
//! `docs/reference/forgejo-backend.md`.

use crate::ids::{parse_issue_id, parse_repository_id, RepoCoord};
use crate::map::map_issue;
use crate::pulls::response_validator;
use crate::types::IssueDto;
use crate::{ForgejoForge, HttpClient, HttpMethod};
use std::cmp::Ordering;
use temper_forge::{
    Comment, CreateComment, CreateIssue, ForgeError, ForgeResult, Issue, IssueId, IssueQuery,
    IssueState, ItemNumber, ItemSortField, RepositoryId, SortDirection, UpdateIssue, UserId,
};

impl<C: HttpClient> ForgejoForge<C> {
    /// Lists issues in a repository, filtered and sorted per `query`.
    ///
    /// Forgejo returns pull requests from `/issues`, so the backend asks for
    /// `type=issues` and additionally drops any row carrying a `pull_request`
    /// marker. The state filter and labels go to the provider; body, author,
    /// and assignee filters are applied client-side after mapping, then the
    /// portable sort.
    pub async fn list_issues(
        &self,
        repo_id: &RepositoryId,
        query: IssueQuery,
    ) -> ForgeResult<Vec<Issue>> {
        let repo = parse_repository_id(repo_id)?;
        let state_param = match query.state {
            None => "all",
            Some(IssueState::Open) => "open",
            Some(IssueState::Closed) => "closed",
        };
        let path = format!("/repos/{}/issues", repo.path_segment());
        let mut base_query = vec![
            ("state".to_string(), state_param.to_string()),
            ("type".to_string(), "issues".to_string()),
        ];
        if !query.labels.is_empty() {
            base_query.push(("labels".to_string(), query.labels.join(",")));
        }
        let dtos: Vec<IssueDto> = self.list_all("list issues", &path, base_query).await?;

        let mut issues: Vec<Issue> = dtos
            .into_iter()
            // Never return a pull request as an issue, even if the provider
            // ignores the `type=issues` filter.
            .filter(|dto| !dto.is_pull_request())
            .map(|dto| self.materialize_issue(&repo, dto, None))
            .collect();
        issues.retain(|issue| issue_matches_query(issue, &query));
        if query.details.dependencies {
            for issue in &mut issues {
                issue.dependencies = self.load_item_dependencies(&repo, issue.number).await?;
            }
        }
        issues.sort_by(|left, right| compare_issues(left, right, &query));
        Ok(issues)
    }

    /// Looks up an issue by stable backend identifier.
    pub async fn get_issue(&self, id: &IssueId) -> ForgeResult<Option<Issue>> {
        let (repo, number) = parse_issue_id(id)?;
        self.fetch_issue_excluding_pulls(&repo, number).await
    }

    /// Looks up an issue by its repository-scoped number.
    ///
    /// A `404` or a PR-as-issue row both map to `Ok(None)`.
    pub async fn get_issue_by_number(
        &self,
        repo_id: &RepositoryId,
        number: ItemNumber,
    ) -> ForgeResult<Option<Issue>> {
        let repo = parse_repository_id(repo_id)?;
        self.fetch_issue_excluding_pulls(&repo, number).await
    }

    /// Creates an issue, then applies labels via the shared issue label helper.
    ///
    /// Assignees are sent in the create payload; labels are applied afterward so
    /// issues and pull requests share one label-sequencing implementation. The
    /// issue is re-fetched so the returned value reflects the applied metadata.
    pub async fn create_issue(
        &self,
        repo_id: &RepositoryId,
        input: CreateIssue,
    ) -> ForgeResult<Issue> {
        let repo = parse_repository_id(repo_id)?;
        let assignees: Vec<&str> = input.assignees.iter().map(UserId::as_str).collect();
        let payload = serde_json::json!({
            "title": input.title,
            "body": input.body,
            "assignees": assignees,
        })
        .to_string();
        let path = format!("/repos/{}/issues", repo.path_segment());
        let response = self
            .request_checked(
                "create issue",
                HttpMethod::Post,
                &path,
                Vec::new(),
                Some(payload),
            )
            .await?;
        let created: IssueDto = Self::decode("create issue", &response)?;
        let number = ItemNumber::new(created.number);

        let set_labels = (!input.labels.is_empty()).then(|| input.labels.clone());
        self.apply_item_label_update(&repo, number, set_labels, Vec::new(), Vec::new())
            .await?;

        self.fetch_issue_excluding_pulls(&repo, number)
            .await?
            .ok_or_else(|| {
                ForgeError::Backend(format!(
                    "issue #{} was not readable after creation",
                    number.get()
                ))
            })
    }

    /// Updates an issue's title/body/state and labels/assignees.
    ///
    /// When `input.expected_version` is set, the current artifact is re-read and
    /// its validator checked against the version cache before any mutation; a
    /// stale token returns [`ForgeError::Conflict`] and mutates nothing. The
    /// label/assignee sequencing matches [`Self::update_pull_request`].
    pub async fn update_issue(&self, id: &IssueId, input: UpdateIssue) -> ForgeResult<Issue> {
        let (repo, number) = parse_issue_id(id)?;
        let path = format!("/repos/{}/issues/{}", repo.path_segment(), number.get());
        let Some(response) = self
            .request_optional("get issue", HttpMethod::Get, &path, Vec::new(), None)
            .await?
        else {
            return Err(ForgeError::NotFound(format!("issue {id}")));
        };
        let validator = response_validator(&response);
        let current_dto: IssueDto = Self::decode("get issue", &response)?;
        // A pull request is never an issue: refuse to mutate it through the
        // issue surface.
        if current_dto.is_pull_request() {
            return Err(ForgeError::NotFound(format!("issue {id}")));
        }
        let current = self.materialize_issue(&repo, current_dto, validator.as_deref());

        if let Some(expected) = input.expected_version {
            self.versions.check(
                id.as_str(),
                validator.as_deref(),
                expected,
                self.config.cas_mode,
            )?;
        }

        let mut edit = serde_json::Map::new();
        if let Some(title) = &input.title {
            edit.insert("title".to_string(), serde_json::json!(title));
        }
        if let Some(body) = &input.body {
            edit.insert("body".to_string(), serde_json::json!(body));
        }
        if let Some(state) = input.state {
            let state = match state {
                IssueState::Open => "open",
                IssueState::Closed => "closed",
            };
            edit.insert("state".to_string(), serde_json::json!(state));
        }
        if !edit.is_empty() {
            let payload = serde_json::Value::Object(edit).to_string();
            self.request_checked(
                "update issue",
                HttpMethod::Patch,
                &path,
                Vec::new(),
                Some(payload),
            )
            .await?;
        }

        self.apply_item_label_update(
            &repo,
            number,
            input.set_labels.clone(),
            input.add_labels.clone(),
            input.remove_labels.clone(),
        )
        .await?;
        self.apply_item_assignee_update(
            &repo,
            number,
            &current.assignees,
            input.add_assignees.clone(),
            input.remove_assignees.clone(),
        )
        .await?;

        self.fetch_issue_excluding_pulls(&repo, number)
            .await?
            .ok_or_else(|| ForgeError::NotFound(format!("issue {id}")))
    }

    /// Lists comments on an issue in chronological order.
    pub async fn list_issue_comments(&self, id: &IssueId) -> ForgeResult<Vec<Comment>> {
        let (repo, number) = parse_issue_id(id)?;
        self.list_item_comments(&repo, number).await
    }

    /// Adds a comment to an issue.
    pub async fn add_issue_comment(
        &self,
        id: &IssueId,
        input: CreateComment,
    ) -> ForgeResult<Comment> {
        let (repo, number) = parse_issue_id(id)?;
        self.add_item_comment(&repo, number, input.body).await
    }

    /// Fetches an issue and enriches it with dependencies, returning `None` for a
    /// missing item or a PR-as-issue row.
    async fn fetch_issue_excluding_pulls(
        &self,
        repo: &RepoCoord,
        number: ItemNumber,
    ) -> ForgeResult<Option<Issue>> {
        let path = format!("/repos/{}/issues/{}", repo.path_segment(), number.get());
        let Some(response) = self
            .request_optional("get issue", HttpMethod::Get, &path, Vec::new(), None)
            .await?
        else {
            return Ok(None);
        };
        let validator = response_validator(&response);
        let dto: IssueDto = Self::decode("get issue", &response)?;
        if dto.is_pull_request() {
            return Ok(None);
        }
        let mut issue = self.materialize_issue(repo, dto, validator.as_deref());
        issue.dependencies = self.load_item_dependencies(repo, number).await?;
        Ok(Some(issue))
    }

    /// Maps an issue DTO and assigns a version from the validator cache.
    ///
    /// The validator is the response `ETag` when present, else the weak
    /// `updated_at` timestamp. Shared by the issue read, update, and
    /// dependency-link paths so version capture lives in one place.
    pub(crate) fn materialize_issue(
        &self,
        repo: &RepoCoord,
        dto: IssueDto,
        etag: Option<&str>,
    ) -> Issue {
        let mut issue = map_issue(repo, dto);
        let validator = etag
            .map(str::to_string)
            .unwrap_or_else(|| issue.updated_at.to_rfc3339());
        issue.version = self.versions.observe(issue.id.as_str(), Some(&validator));
        issue
    }
}

/// Returns whether an issue satisfies the client-side filters of `query`.
///
/// State and labels are filtered by the provider; body, author, and assignee
/// are checked here after the provider has already narrowed the result.
fn issue_matches_query(issue: &Issue, query: &IssueQuery) -> bool {
    if let Some(needle) = &query.body_contains {
        if !needle.is_empty() && !issue.body.contains(needle) {
            return false;
        }
    }
    if let Some(author) = &query.author_id {
        if &issue.author_id != author {
            return false;
        }
    }
    if let Some(assignee) = &query.assignee_id {
        if !issue
            .assignees
            .iter()
            .any(|candidate| candidate == assignee)
        {
            return false;
        }
    }
    true
}

/// Orders issues by the requested sort, then by number, then by id.
fn compare_issues(left: &Issue, right: &Issue, query: &IssueQuery) -> Ordering {
    let primary = query
        .sort
        .map(|sort| {
            let comparison = match sort.field {
                ItemSortField::Number => left.number.cmp(&right.number),
                ItemSortField::CreatedAt => left.created_at.cmp(&right.created_at),
                ItemSortField::UpdatedAt => left.updated_at.cmp(&right.updated_at),
            };
            match sort.direction {
                SortDirection::Asc => comparison,
                SortDirection::Desc => comparison.reverse(),
            }
        })
        .unwrap_or_else(|| left.number.cmp(&right.number));
    primary
        .then_with(|| left.number.cmp(&right.number))
        .then_with(|| left.id.cmp(&right.id))
}
