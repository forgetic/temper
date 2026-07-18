//! Forgejo pull-request operations: list/get/create/update, comments, and
//! merge. Reviewer requests and native reviews live in [`reviews`]; list paths
//! in [`list`]. Label and assignee changes reuse the shared item helpers in
//! [`crate::items`]. See `docs/reference/forgejo-backend.md`.

use crate::ids::{RepoCoord, parse_pull_request_id, parse_repository_id};
use crate::items::coalesce_label_update;
use crate::map::{map_pull_request, merge_method_token};
use crate::types::PullRequestDto;
use crate::{ForgejoForge, HttpClient, HttpMethod};
use temper_forge_model::{
    Comment, CreateComment, CreatePullRequest, ForgeError, ForgeResult, ItemListDetails,
    ItemNumber, MergePullRequest, MergeRecord, PullRequest, PullRequestId, PullRequestUpdateState,
    RepositoryId, UpdatePullRequest,
};

mod list;
mod reviews;

impl<C: HttpClient> ForgejoForge<C> {
    /// Looks up a pull request by stable backend identifier.
    pub async fn get_pull_request(&self, id: &PullRequestId) -> ForgeResult<Option<PullRequest>> {
        self.get_pull_request_with_details(id, ItemListDetails::full())
            .await
    }

    /// Looks up a pull request by stable identifier with an explicit detail
    /// budget. Summary reads use only `/pulls/{number}`; full reads additionally
    /// load the shared issue dependency endpoint.
    pub async fn get_pull_request_with_details(
        &self,
        id: &PullRequestId,
        details: ItemListDetails,
    ) -> ForgeResult<Option<PullRequest>> {
        let (repo, number) = parse_pull_request_id(id)?;
        self.fetch_pull_request_with_details(&repo, number, details)
            .await
    }

    /// Looks up a pull request by its repository-scoped number.
    pub async fn get_pull_request_by_number(
        &self,
        repo_id: &RepositoryId,
        number: ItemNumber,
    ) -> ForgeResult<Option<PullRequest>> {
        self.get_pull_request_by_number_with_details(repo_id, number, ItemListDetails::full())
            .await
    }

    /// Looks up a pull request by number with an explicit detail budget.
    pub async fn get_pull_request_by_number_with_details(
        &self,
        repo_id: &RepositoryId,
        number: ItemNumber,
        details: ItemListDetails,
    ) -> ForgeResult<Option<PullRequest>> {
        let repo = parse_repository_id(repo_id)?;
        self.fetch_pull_request_with_details(&repo, number, details)
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

        let (set_labels, add_labels, remove_labels) = coalesce_label_update(
            &current.labels,
            input.set_labels,
            input.add_labels,
            input.remove_labels,
        );
        self.apply_item_label_update(&repo, number, set_labels, add_labels, remove_labels)
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
        if input.delete_source_branch {
            body.insert(
                "delete_branch_after_merge".to_string(),
                serde_json::json!(true),
            );
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
        self.fetch_pull_request_with_details(repo, number, ItemListDetails::full())
            .await
    }

    /// Fetches a pull request and enriches only the requested detail fields.
    pub(crate) async fn fetch_pull_request_with_details(
        &self,
        repo: &RepoCoord,
        number: ItemNumber,
        details: ItemListDetails,
    ) -> ForgeResult<Option<PullRequest>> {
        let Some(mut pull) = self.fetch_pull_request(repo, number).await? else {
            return Ok(None);
        };
        if details.dependencies {
            pull.dependencies = self.load_item_dependencies(repo, number).await?;
        }
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
