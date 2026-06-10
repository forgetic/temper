//! GitHub pull-request operations: list/get/create/update, comments,
//! reviewers, native reviews, and merge. Label and assignee changes reuse the
//! shared item helpers in [`crate::items`].

use crate::ids::{
    format_review_id, format_user_id, parse_pull_request_id, parse_repository_id, RepoCoord,
};
use crate::map::{map_pull_request, map_review, merge_method_token, review_event_token};
use crate::types::{MergeResultDto, PullRequestDto, ReviewDto};
use crate::{GitHubForge, HttpClient, HttpMethod};
use chrono::Utc;
use std::cmp::Ordering;
use temper_forge::{
    Comment, CreateComment, CreatePullRequest, CreatePullRequestReview, ForgeError, ForgeResult,
    ItemNumber, ItemSortField, MergePullRequest, MergeRecord, PullRequest, PullRequestId,
    PullRequestQuery, PullRequestReview, PullRequestState, PullRequestUpdateState, RepositoryId,
    RequestReviewers, SortDirection, UpdatePullRequest,
};

impl<C: HttpClient> GitHubForge<C> {
    /// Lists pull requests in a repository, filtered and sorted per `query`.
    ///
    /// GitHub's `/pulls` list rows already carry labels, assignees, and
    /// requested reviewers, so one provider-side state filter suffices and the
    /// remaining filters (labels, body, author, assignee) are applied
    /// client-side. List rows omit the `merged` boolean but include
    /// `merged_at`, which the mapping uses to distinguish merged from closed.
    pub async fn list_pull_requests(
        &self,
        repo_id: &RepositoryId,
        query: PullRequestQuery,
    ) -> ForgeResult<Vec<PullRequest>> {
        let repo = parse_repository_id(repo_id)?;
        let path = format!("/repos/{}/pulls", repo.path_segment());
        let base_query = vec![(
            "state".to_string(),
            pull_request_state_param(query.state).to_string(),
        )];
        let dtos: Vec<PullRequestDto> = self
            .list_all("list pull requests", &path, base_query)
            .await?;
        let mut pulls: Vec<PullRequest> = dtos
            .into_iter()
            .map(|dto| self.materialize_pull_request(&repo, dto, None))
            .collect();
        pulls.retain(|pull| pull_matches_query(pull, &query));
        // GitHub exposes no native dependency links, so `query.details` needs
        // no enrichment pass: `dependencies` is always empty.
        pulls.sort_by(|left, right| compare_pull_requests(left, right, &query));
        Ok(pulls)
    }

    /// Looks up a pull request by stable backend identifier.
    pub async fn get_pull_request(&self, id: &PullRequestId) -> ForgeResult<Option<PullRequest>> {
        let (repo, number) = parse_pull_request_id(id)?;
        self.fetch_pull_request(&repo, number).await
    }

    /// Looks up a pull request by its repository-scoped number.
    pub async fn get_pull_request_by_number(
        &self,
        repo_id: &RepositoryId,
        number: ItemNumber,
    ) -> ForgeResult<Option<PullRequest>> {
        let repo = parse_repository_id(repo_id)?;
        self.fetch_pull_request(&repo, number).await
    }

    /// Creates a pull request, then applies labels/assignees via issue endpoints.
    ///
    /// GitHub's create payload accepts neither labels nor assignees, so both
    /// are applied through the shared item helpers and the pull request is
    /// re-fetched to reflect the applied metadata.
    pub async fn create_pull_request(
        &self,
        repo_id: &RepositoryId,
        input: CreatePullRequest,
    ) -> ForgeResult<PullRequest> {
        let repo = parse_repository_id(repo_id)?;
        let payload = serde_json::json!({
            "title": input.title,
            "head": input.source.branch,
            "base": input.target.branch,
            "body": input.body,
        })
        .to_string();
        let path = format!("/repos/{}/pulls", repo.path_segment());
        let response = self
            .request_checked(
                "create pull request",
                HttpMethod::Post,
                &path,
                Vec::new(),
                Some(payload),
            )
            .await?;
        let created: PullRequestDto = Self::decode("create pull request", &response)?;
        let number = ItemNumber::new(created.number);

        let set_labels = (!input.labels.is_empty()).then(|| input.labels.clone());
        self.apply_item_label_update(&repo, number, set_labels, Vec::new(), Vec::new())
            .await?;
        self.apply_item_assignee_update(&repo, number, &[], input.assignees.clone(), Vec::new())
            .await?;

        self.fetch_pull_request(&repo, number)
            .await?
            .ok_or_else(|| {
                ForgeError::Backend(format!(
                    "pull request #{} was not readable after creation",
                    number.get()
                ))
            })
    }

    /// Updates a pull request's title/body/state and labels/assignees.
    ///
    /// When `input.expected_version` is set, the current artifact is re-read and
    /// its validator checked against the version cache before any mutation; a
    /// stale token returns [`ForgeError::Conflict`] and mutates nothing.
    pub async fn update_pull_request(
        &self,
        id: &PullRequestId,
        input: UpdatePullRequest,
    ) -> ForgeResult<PullRequest> {
        let (repo, number) = parse_pull_request_id(id)?;
        let path = format!("/repos/{}/pulls/{}", repo.path_segment(), number.get());
        let Some(response) = self
            .request_optional("get pull request", HttpMethod::Get, &path, Vec::new(), None)
            .await?
        else {
            return Err(ForgeError::NotFound(format!("pull request {id}")));
        };
        let validator = response_validator(&response);
        let current_dto: PullRequestDto = Self::decode("get pull request", &response)?;
        let current = self.materialize_pull_request(&repo, current_dto, validator.as_deref());

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
                PullRequestUpdateState::Open => "open",
                PullRequestUpdateState::Closed => "closed",
            };
            edit.insert("state".to_string(), serde_json::json!(state));
        }
        if !edit.is_empty() {
            let payload = serde_json::Value::Object(edit).to_string();
            self.request_checked(
                "update pull request",
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

        self.fetch_pull_request(&repo, number)
            .await?
            .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))
    }

    /// Requests reviews from users; idempotent when the set already matches.
    pub async fn request_pull_request_reviewers(
        &self,
        id: &PullRequestId,
        input: RequestReviewers,
    ) -> ForgeResult<PullRequest> {
        let (repo, number) = parse_pull_request_id(id)?;
        let reviewers: Vec<&str> = input
            .reviewers
            .iter()
            .map(temper_forge::UserId::as_str)
            .collect();
        let path = format!(
            "/repos/{}/pulls/{}/requested_reviewers",
            repo.path_segment(),
            number.get()
        );
        let payload = serde_json::json!({ "reviewers": reviewers }).to_string();
        let response = self
            .send(HttpMethod::Post, &path, Vec::new(), Some(payload))
            .await?;

        if response.is_success() {
            return self
                .fetch_pull_request(&repo, number)
                .await?
                .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")));
        }
        if response.status == 404 {
            return Err(ForgeError::NotFound(format!("pull request {id}")));
        }

        // GitHub rejects some re-request shapes (e.g. a reviewer who is the
        // author, or one already requested on older Enterprise versions) with
        // `422`. Treat the call as idempotent when the desired reviewers are
        // already present.
        if let Some(pull) = self.fetch_pull_request(&repo, number).await? {
            if input
                .reviewers
                .iter()
                .all(|reviewer| pull.requested_reviewers.contains(reviewer))
            {
                return Ok(pull);
            }
        }
        Err(crate::error::map_status_error(
            "request reviewers",
            &response,
        ))
    }

    /// Lists native review verdicts in chronological order.
    ///
    /// Dismissed reviews are dropped: GitHub's `DISMISSED` state replaces the
    /// original verdict, so the event carries no decision to report (see
    /// [`map_review`](crate::map::map_review)).
    pub async fn list_pull_request_reviews(
        &self,
        id: &PullRequestId,
    ) -> ForgeResult<Vec<PullRequestReview>> {
        let (repo, number) = parse_pull_request_id(id)?;
        let path = format!(
            "/repos/{}/pulls/{}/reviews",
            repo.path_segment(),
            number.get()
        );
        let dtos: Vec<ReviewDto> = self.list_all("list reviews", &path, Vec::new()).await?;
        let mut reviews: Vec<PullRequestReview> = dtos
            .into_iter()
            .filter_map(|dto| map_review(&repo, id, dto))
            .collect();
        reviews.sort_by(|left, right| {
            left.submitted_at
                .cmp(&right.submitted_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(reviews)
    }

    /// Submits a native review in one call. Pending is rejected as invalid.
    pub async fn submit_pull_request_review(
        &self,
        id: &PullRequestId,
        input: CreatePullRequestReview,
    ) -> ForgeResult<PullRequestReview> {
        let (repo, number) = parse_pull_request_id(id)?;
        let event = review_event_token(input.decision)?;
        let body = input.body.clone().unwrap_or_default();
        let path = format!(
            "/repos/{}/pulls/{}/reviews",
            repo.path_segment(),
            number.get()
        );
        let payload = serde_json::json!({ "event": event, "body": body }).to_string();
        let response = self
            .request_checked(
                "submit review",
                HttpMethod::Post,
                &path,
                Vec::new(),
                Some(payload),
            )
            .await?;
        let dto: ReviewDto = Self::decode("submit review", &response)?;

        if let Some(review) = map_review(&repo, id, dto.clone()) {
            return Ok(review);
        }
        // Fall back to the decision we submitted if the provider echo is sparse.
        let submitted_at = dto.submitted_at.unwrap_or_else(Utc::now);
        Ok(PullRequestReview {
            id: format_review_id(&repo, dto.id),
            pull_request_id: id.clone(),
            reviewer_id: format_user_id(&dto.user.login),
            decision: input.decision,
            body: input.body,
            submitted_at,
        })
    }

    /// Lists comments on a pull request (GitHub issue comments).
    pub async fn list_pull_request_comments(
        &self,
        id: &PullRequestId,
    ) -> ForgeResult<Vec<Comment>> {
        let (repo, number) = parse_pull_request_id(id)?;
        self.list_item_comments(&repo, number).await
    }

    /// Adds a comment to a pull request (GitHub issue comment).
    pub async fn add_pull_request_comment(
        &self,
        id: &PullRequestId,
        input: CreateComment,
    ) -> ForgeResult<Comment> {
        let (repo, number) = parse_pull_request_id(id)?;
        self.add_item_comment(&repo, number, input.body).await
    }

    /// Merges a pull request and returns the recorded merge metadata.
    ///
    /// The merge `PUT` returns only the merge commit SHA, so the pull request
    /// is re-fetched for the merger and timestamp. Conflict-like statuses
    /// (`405` not mergeable, `409` head mismatch, plus `412`/`422`) map to
    /// [`ForgeError::Conflict`].
    pub async fn merge_pull_request(
        &self,
        id: &PullRequestId,
        input: MergePullRequest,
    ) -> ForgeResult<MergeRecord> {
        let (repo, number) = parse_pull_request_id(id)?;
        let mut body = serde_json::Map::new();
        body.insert(
            "merge_method".to_string(),
            serde_json::json!(merge_method_token(input.method)),
        );
        if let Some(title) = &input.commit_title {
            body.insert("commit_title".to_string(), serde_json::json!(title));
        }
        if let Some(message) = &input.commit_body {
            body.insert("commit_message".to_string(), serde_json::json!(message));
        }
        let path = format!(
            "/repos/{}/pulls/{}/merge",
            repo.path_segment(),
            number.get()
        );
        let payload = serde_json::Value::Object(body).to_string();
        let response = self
            .send(HttpMethod::Put, &path, Vec::new(), Some(payload))
            .await?;

        if !response.is_success() {
            return Err(match response.status {
                404 => ForgeError::NotFound(format!("pull request {id}")),
                405 | 409 | 412 | 422 => ForgeError::Conflict(format!(
                    "merge pull request {id}: HTTP {}{}",
                    response.status,
                    snippet(&response.body)
                )),
                _ => crate::error::map_status_error("merge pull request", &response),
            });
        }
        // Decoded for shape validation; the merge record below is rebuilt from
        // the re-fetched pull request so all fields share one source.
        let _merge: MergeResultDto = Self::decode("merge pull request", &response)?;

        let pull = self
            .fetch_pull_request(&repo, number)
            .await?
            .ok_or_else(|| ForgeError::NotFound(format!("pull request {id}")))?;
        let merge = pull.merge.ok_or_else(|| {
            ForgeError::Backend(format!(
                "merge of pull request {id} reported success but no merge commit was returned"
            ))
        })?;
        Ok(MergeRecord {
            method: input.method,
            ..merge
        })
    }

    /// Fetches a single pull request, returning `None` on `404`.
    pub(crate) async fn fetch_pull_request(
        &self,
        repo: &RepoCoord,
        number: ItemNumber,
    ) -> ForgeResult<Option<PullRequest>> {
        let path = format!("/repos/{}/pulls/{}", repo.path_segment(), number.get());
        let Some(response) = self
            .request_optional("get pull request", HttpMethod::Get, &path, Vec::new(), None)
            .await?
        else {
            return Ok(None);
        };
        let validator = response_validator(&response);
        let dto: PullRequestDto = Self::decode("get pull request", &response)?;
        Ok(Some(self.materialize_pull_request(
            repo,
            dto,
            validator.as_deref(),
        )))
    }

    /// Maps a pull-request DTO and assigns a version from the validator cache.
    fn materialize_pull_request(
        &self,
        repo: &RepoCoord,
        dto: PullRequestDto,
        etag: Option<&str>,
    ) -> PullRequest {
        let mut pull = map_pull_request(repo, dto);
        let validator = etag
            .map(str::to_string)
            .unwrap_or_else(|| pull.updated_at.to_rfc3339());
        pull.version = self.versions.observe(pull.id.as_str(), Some(&validator));
        pull
    }
}

/// Returns the response validator: the `ETag` header if present, else `None`
/// (callers fall back to `updated_at`).
pub(crate) fn response_validator(response: &crate::HttpResponse) -> Option<String> {
    response.header("etag").map(str::to_string)
}

/// Truncates a provider error body for inclusion in an error message.
fn snippet(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let snippet: String = trimmed.chars().take(200).collect();
    format!(": {snippet}")
}

fn pull_request_state_param(state: Option<PullRequestState>) -> &'static str {
    match state {
        None => "all",
        Some(PullRequestState::Open) => "open",
        // GitHub maps both portable Closed and Merged to provider `closed`;
        // the client-side filter separates them after mapping.
        Some(PullRequestState::Closed) | Some(PullRequestState::Merged) => "closed",
    }
}

fn pull_matches_query(pull: &PullRequest, query: &PullRequestQuery) -> bool {
    if let Some(state) = query.state {
        if pull.state != state {
            return false;
        }
    }
    if !query
        .labels
        .iter()
        .all(|required| pull.labels.iter().any(|label| label == required))
    {
        return false;
    }
    if let Some(needle) = &query.body_contains {
        if !needle.is_empty() && !pull.body.contains(needle) {
            return false;
        }
    }
    if let Some(author) = &query.author_id {
        if &pull.author_id != author {
            return false;
        }
    }
    if let Some(assignee) = &query.assignee_id {
        if !pull.assignees.iter().any(|candidate| candidate == assignee) {
            return false;
        }
    }
    true
}

fn compare_pull_requests(
    left: &PullRequest,
    right: &PullRequest,
    query: &PullRequestQuery,
) -> Ordering {
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
