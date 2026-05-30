//! Pull-request operations: list/get/create/update, comments, reviewer
//! requests, native reviews, and merge.
//!
//! These are inherent methods on [`ForgejoForge`] mirroring the
//! [`harness_forge::Forge`] pull-request surface; the trait implementation is
//! assembled once every phase's methods exist. Label and assignee changes reuse
//! the shared item helpers in [`crate::items`] so pull requests and issues share
//! one sequencing implementation. See `docs/reference/forgejo-backend.md`.

use crate::ids::{
    format_review_id, format_user_id, parse_pull_request_id, parse_repository_id, RepoCoord,
};
use crate::map::{map_pull_request, map_review, merge_method_token, review_event_token};
use crate::types::{PullRequestDto, ReviewDto};
use crate::{ForgejoForge, HttpClient, HttpMethod};
use chrono::Utc;
use harness_forge::{
    Comment, CreateComment, CreatePullRequest, CreatePullRequestReview, ForgeError, ForgeResult,
    ItemNumber, ItemSortField, MergePullRequest, MergeRecord, PullRequest, PullRequestId,
    PullRequestQuery, PullRequestReview, PullRequestState, PullRequestUpdateState, RepositoryId,
    RequestReviewers, SortDirection, UpdatePullRequest,
};
use std::cmp::Ordering;

impl<C: HttpClient> ForgejoForge<C> {
    /// Lists pull requests in a repository, filtered and sorted per `query`.
    ///
    /// Forgejo's `/pulls` endpoint does not filter by label, so the backend
    /// fetches by state and applies label/author/assignee filters and the exact
    /// portable state filter client-side after mapping.
    pub async fn list_pull_requests(
        &self,
        repo_id: &RepositoryId,
        query: PullRequestQuery,
    ) -> ForgeResult<Vec<PullRequest>> {
        let repo = parse_repository_id(repo_id)?;
        let state_param = match query.state {
            None => "all",
            Some(PullRequestState::Open) => "open",
            Some(PullRequestState::Closed) | Some(PullRequestState::Merged) => "closed",
        };
        let path = format!("/repos/{}/pulls", repo.path_segment());
        let base_query = vec![("state".to_string(), state_param.to_string())];
        let dtos: Vec<PullRequestDto> = self
            .list_all("list pull requests", &path, base_query)
            .await?;

        let mut pulls: Vec<PullRequest> = dtos
            .into_iter()
            .map(|dto| self.materialize_pull_request(&repo, dto, None))
            .collect();
        pulls.retain(|pull| pull_matches_query(pull, &query));
        // Enrich the matching pull requests with their dependency links. This is
        // an N+1 read against the dependencies endpoint, acceptable for a first
        // best-effort backend; the helper is isolated for later batching.
        for pull in &mut pulls {
            pull.dependencies = self.load_item_dependencies(&repo, pull.number).await?;
        }
        pulls.sort_by(|left, right| compare_pull_requests(left, right, &query));
        Ok(pulls)
    }

    /// Looks up a pull request by stable backend identifier.
    pub async fn get_pull_request(&self, id: &PullRequestId) -> ForgeResult<Option<PullRequest>> {
        let (repo, number) = parse_pull_request_id(id)?;
        self.fetch_pull_request_with_dependencies(&repo, number)
            .await
    }

    /// Looks up a pull request by its repository-scoped number.
    pub async fn get_pull_request_by_number(
        &self,
        repo_id: &RepositoryId,
        number: ItemNumber,
    ) -> ForgeResult<Option<PullRequest>> {
        let repo = parse_repository_id(repo_id)?;
        self.fetch_pull_request_with_dependencies(&repo, number)
            .await
    }

    /// Creates a pull request, then applies labels/assignees via issue endpoints.
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
            .map(harness_forge::UserId::as_str)
            .collect();
        let path = format!(
            "/repos/{}/pulls/{}/requested_reviewers",
            repo.path_segment(),
            number.get()
        );
        let payload =
            serde_json::json!({ "reviewers": reviewers, "team_reviewers": [] }).to_string();
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

        // Forgejo rejects re-requesting an already-requested reviewer. Treat the
        // call as idempotent when the desired reviewers are already present.
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

    /// Lists native review verdicts in chronological order, skipping only
    /// non-verdict (review-request/pending) events. Dismissed and stale verdicts
    /// are **kept** so history (e.g. a changes-requested review later auto-
    /// dismissed by an approval) matches the reference backends; see
    /// [`map_review`](crate::map::map_review).
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
        let submitted_at = dto
            .submitted_at
            .or(dto.updated_at)
            .or(dto.created_at)
            .unwrap_or_else(Utc::now);
        Ok(PullRequestReview {
            id: format_review_id(&repo, dto.id),
            pull_request_id: id.clone(),
            reviewer_id: format_user_id(&dto.user.login),
            decision: input.decision,
            body: input.body,
            submitted_at,
        })
    }

    /// Lists comments on a pull request (Forgejo issue comments).
    pub async fn list_pull_request_comments(
        &self,
        id: &PullRequestId,
    ) -> ForgeResult<Vec<Comment>> {
        let (repo, number) = parse_pull_request_id(id)?;
        self.list_item_comments(&repo, number).await
    }

    /// Adds a comment to a pull request (Forgejo issue comment).
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
    /// The merge `POST` returns no body, so the pull request is re-fetched for
    /// the merge commit SHA, merger, and timestamp. Conflict-like statuses
    /// (already merged, not mergeable) map to [`ForgeError::Conflict`].
    pub async fn merge_pull_request(
        &self,
        id: &PullRequestId,
        input: MergePullRequest,
    ) -> ForgeResult<MergeRecord> {
        let (repo, number) = parse_pull_request_id(id)?;
        let mut body = serde_json::Map::new();
        body.insert(
            "Do".to_string(),
            serde_json::json!(merge_method_token(input.method)),
        );
        if let Some(title) = &input.commit_title {
            body.insert("MergeTitleField".to_string(), serde_json::json!(title));
        }
        if let Some(message) = &input.commit_body {
            body.insert("MergeMessageField".to_string(), serde_json::json!(message));
        }
        let path = format!(
            "/repos/{}/pulls/{}/merge",
            repo.path_segment(),
            number.get()
        );
        let payload = serde_json::Value::Object(body).to_string();
        let response = self
            .send(HttpMethod::Post, &path, Vec::new(), Some(payload))
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

    /// Fetches a pull request and enriches it with its dependency links.
    ///
    /// Used by the read paths and the dependency-link methods so a returned
    /// pull request always carries its dependencies. The internal
    /// [`Self::fetch_pull_request`] used by mutation paths (create/update/merge/
    /// reviewer requests) deliberately skips the extra dependency read.
    pub(crate) async fn fetch_pull_request_with_dependencies(
        &self,
        repo: &RepoCoord,
        number: ItemNumber,
    ) -> ForgeResult<Option<PullRequest>> {
        let Some(mut pull) = self.fetch_pull_request(repo, number).await? else {
            return Ok(None);
        };
        pull.dependencies = self.load_item_dependencies(repo, number).await?;
        Ok(Some(pull))
    }

    /// Fetches a single pull request, returning `None` on `404`.
    async fn fetch_pull_request(
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
