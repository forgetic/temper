//! Issue operations: list/get/create/update plus issue comments.
//!
//! These are inherent methods on [`GitHubForge`] mirroring the
//! [`temper_forge_model::Forge`] issue surface; the trait implementation is assembled
//! in [`crate::forge_impl`]. GitHub serves both issues and pull requests
//! through the issue endpoints and offers no `type` filter, so the read paths
//! drop every row carrying a `pull_request` marker (a pull request is never
//! returned as an [`Issue`]). Label and assignee changes reuse the shared item
//! helpers in [`crate::items`] so issues and pull requests share one sequencing
//! implementation. The conditional-update path mirrors
//! [`crate::pulls::GitHubForge::update_pull_request`].

use crate::ids::{RepoCoord, parse_issue_id, parse_repository_id};
use crate::map::map_issue;
use crate::pulls::response_validator;
use crate::types::IssueDto;
use crate::{GitHubForge, HttpClient, HttpMethod};
use std::cmp::Ordering;
use std::collections::BTreeSet;
use temper_forge_model::{
    Comment, CreateComment, CreateIssue, ForgeError, ForgeResult, Issue, IssueId, IssueQuery,
    IssueState, ItemListDetails, ItemNumber, ItemSortField, RepositoryId, SortDirection,
    UpdateIssue, UserId,
};

impl<C: HttpClient> GitHubForge<C> {
    /// Lists issues in a repository, filtered and sorted per `query`.
    ///
    /// GitHub returns pull requests from `/issues` and has no `type` filter, so
    /// the backend drops any row carrying a `pull_request` marker. The state
    /// filter and labels go to the provider; body, author, and assignee filters
    /// are applied client-side after mapping, then the portable sort.
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
        let mut base_query = vec![("state".to_string(), state_param.to_string())];
        if !query.labels.is_empty() {
            base_query.push(("labels".to_string(), query.labels.join(",")));
        }
        let dtos: Vec<IssueDto> = self
            .list_up_to("list issues", &path, base_query, query.limit)
            .await?;

        let mut issues: Vec<Issue> = dtos
            .into_iter()
            // Never return a pull request as an issue.
            .filter(|dto| !dto.is_pull_request())
            .map(|dto| self.materialize_issue(&repo, dto, None))
            .collect();
        issues.retain(|issue| issue_matches_query(issue, &query));
        // GitHub exposes no native dependency links, so `query.details` needs
        // no enrichment pass: `dependencies` is always empty.
        issues.sort_by(|left, right| compare_issues(left, right, &query));
        if let Some(limit) = query.limit {
            issues.truncate(limit);
        }
        Ok(issues)
    }

    /// Looks up an issue by stable backend identifier.
    pub async fn get_issue(&self, id: &IssueId) -> ForgeResult<Option<Issue>> {
        let (repo, number) = parse_issue_id(id)?;
        self.fetch_issue_excluding_pulls(&repo, number).await
    }

    /// Looks up an issue by id with an explicit detail budget. GitHub has no
    /// native dependency enrichment, so both variants use one provider read.
    pub async fn get_issue_with_details(
        &self,
        id: &IssueId,
        _details: ItemListDetails,
    ) -> ForgeResult<Option<Issue>> {
        self.get_issue(id).await
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

    /// Looks up an issue by number with an explicit detail budget.
    pub async fn get_issue_by_number_with_details(
        &self,
        repo_id: &RepositoryId,
        number: ItemNumber,
        _details: ItemListDetails,
    ) -> ForgeResult<Option<Issue>> {
        self.get_issue_by_number(repo_id, number).await
    }

    /// Creates an issue with its labels and assignees in one call.
    ///
    /// GitHub's create payload accepts label names and assignee logins
    /// directly, so no follow-up label sequencing is needed. The issue is
    /// re-fetched so the returned value reflects the applied metadata.
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
            "labels": input.labels,
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

    /// Updates an issue from a validated snapshot without unconditional
    /// read-before/write-after requests. A conditional update retains one
    /// provider preflight, and label changes remain ordered before a body edit
    /// that may clear workflow staging.
    pub async fn update_issue_from_snapshot(
        &self,
        current: &Issue,
        input: UpdateIssue,
    ) -> ForgeResult<Issue> {
        let (repo, number) = parse_issue_id(&current.id)?;
        if current.repo_id != crate::ids::format_repository_id(&repo) {
            return Err(ForgeError::InvalidRequest(format!(
                "issue snapshot {} has mismatched repository {}",
                current.id, current.repo_id
            )));
        }

        if let Some(expected) = input.expected_version {
            if expected != current.version {
                return Err(ForgeError::Conflict(format!(
                    "stale conditional update of {}: expected version {expected}, snapshot is {}",
                    current.id, current.version
                )));
            }
            let path = format!("/repos/{}/issues/{}", repo.path_segment(), number.get());
            let Some(response) = self
                .request_optional("get issue", HttpMethod::Get, &path, Vec::new(), None)
                .await?
            else {
                return Err(ForgeError::NotFound(format!("issue {}", current.id)));
            };
            let validator = response_validator(&response);
            let dto: IssueDto = Self::decode("get issue", &response)?;
            if dto.is_pull_request() {
                return Err(ForgeError::NotFound(format!("issue {}", current.id)));
            }
            self.versions.check(
                current.id.as_str(),
                validator.as_deref(),
                expected,
                self.config.cas_mode,
            )?;
        }

        let UpdateIssue {
            title,
            body,
            state,
            set_labels,
            add_labels,
            remove_labels,
            add_assignees,
            remove_assignees,
            expected_version: _,
        } = input;

        // Labels must be externally final before a body update can clear
        // metadata.staged.
        self.apply_item_label_update(
            &repo,
            number,
            set_labels.clone(),
            add_labels.clone(),
            remove_labels.clone(),
        )
        .await?;
        self.apply_item_assignee_update(
            &repo,
            number,
            &current.assignees,
            add_assignees.clone(),
            remove_assignees.clone(),
        )
        .await?;

        let mut committed = current.clone();
        apply_committed_labels(&mut committed.labels, set_labels, remove_labels, add_labels);
        apply_committed_assignees(&mut committed.assignees, remove_assignees, add_assignees);

        let mut edit = serde_json::Map::new();
        if let Some(title) = &title {
            edit.insert("title".to_string(), serde_json::json!(title));
        }
        if let Some(body) = &body {
            edit.insert("body".to_string(), serde_json::json!(body));
        }
        if let Some(state) = state {
            let state = match state {
                IssueState::Open => "open",
                IssueState::Closed => "closed",
            };
            edit.insert("state".to_string(), serde_json::json!(state));
        }
        if edit.is_empty() {
            committed.version = self
                .versions
                .commit(current.id.as_str(), None, current.version);
            return Ok(committed);
        }

        let path = format!("/repos/{}/issues/{}", repo.path_segment(), number.get());
        let response = self
            .request_checked(
                "update issue",
                HttpMethod::Patch,
                &path,
                Vec::new(),
                Some(serde_json::Value::Object(edit).to_string()),
            )
            .await?;
        let validator = response_validator(&response);
        let dto: IssueDto = Self::decode("update issue", &response)?;
        if dto.is_pull_request() {
            return Err(ForgeError::NotFound(format!("issue {}", current.id)));
        }
        let dependencies = committed.dependencies.clone();
        let final_labels = committed.labels;
        let final_assignees = committed.assignees;
        committed = crate::map::map_issue(&repo, dto);
        committed.dependencies = dependencies;
        committed.labels = final_labels;
        committed.assignees = final_assignees;
        let fallback_validator = committed.updated_at.to_rfc3339();
        committed.version = self.versions.commit(
            committed.id.as_str(),
            validator.as_deref().or(Some(fallback_validator.as_str())),
            current.version,
        );
        Ok(committed)
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

    /// Fetches an issue, returning `None` for a missing item or a PR-as-issue
    /// row.
    pub(crate) async fn fetch_issue_excluding_pulls(
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
        Ok(Some(self.materialize_issue(
            repo,
            dto,
            validator.as_deref(),
        )))
    }

    /// Maps an issue DTO and assigns a version from the validator cache.
    ///
    /// The validator is the response `ETag` when present, else the weak
    /// `updated_at` timestamp. Shared by the issue read and update paths so
    /// version capture lives in one place.
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

fn sorted_strings(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn apply_committed_labels(
    current: &mut Vec<String>,
    set: Option<Vec<String>>,
    remove: Vec<String>,
    add: Vec<String>,
) {
    let mut labels = set.unwrap_or_else(|| current.clone());
    labels.retain(|label| !remove.contains(label));
    labels.extend(add);
    *current = sorted_strings(labels);
}

fn apply_committed_assignees(current: &mut Vec<UserId>, remove: Vec<UserId>, add: Vec<UserId>) {
    current.retain(|user| !remove.contains(user));
    current.extend(add);
    current.sort();
    current.dedup();
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
