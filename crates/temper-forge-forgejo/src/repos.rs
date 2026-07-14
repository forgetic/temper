//! Identity, repository, and label operations.
//!
//! These are inherent methods on [`ForgejoForge`] mirroring the
//! [`temper_forge_model::Forge`] identity/repository/label surface; the trait
//! implementation is assembled once every phase's methods exist. Pure DTO→model
//! conversions live in [`crate::map`]. See `docs/reference/forgejo-backend.md`.

use crate::ids::{RepoCoord, parse_repository_id, user_login};
use crate::map::{map_label, map_repository, map_user};
use crate::types::{LabelDto, RepositoryDto, UserDto};
use crate::{ForgejoForge, HttpClient, HttpMethod};
use std::cmp::Ordering;
use temper_forge_model::{
    CreateRepository, ForgeError, ForgeResult, Label, Repository, RepositoryId, RepositoryPath,
    RepositoryQuery, RepositorySortField, SortDirection, UpsertLabel, User, UserId,
};

impl<C: HttpClient> ForgejoForge<C> {
    // --- identity ----------------------------------------------------------

    /// Returns the user identity backing this client (`GET /user`).
    pub async fn current_user(&self) -> ForgeResult<User> {
        let response = self
            .request_checked("current user", HttpMethod::Get, "/user", Vec::new(), None)
            .await?;
        let dto: UserDto = Self::decode("current user", &response)?;
        Ok(map_user(dto))
    }

    /// Looks up a user by login (`GET /users/{login}`); `404` maps to `None`.
    pub async fn get_user(&self, id: &UserId) -> ForgeResult<Option<User>> {
        let login = user_login(id)?;
        let path = format!("/users/{login}");
        let Some(response) = self
            .request_optional("get user", HttpMethod::Get, &path, Vec::new(), None)
            .await?
        else {
            return Ok(None);
        };
        let dto: UserDto = Self::decode("get user", &response)?;
        Ok(Some(map_user(dto)))
    }

    // --- repositories ------------------------------------------------------

    /// Looks up a repository by owner/name path; `404` maps to `None`.
    pub async fn get_repository_by_path(
        &self,
        path: &RepositoryPath,
    ) -> ForgeResult<Option<Repository>> {
        let api_path = format!("/repos/{}/{}", path.owner, path.name);
        let Some(response) = self
            .request_optional(
                "get repository",
                HttpMethod::Get,
                &api_path,
                Vec::new(),
                None,
            )
            .await?
        else {
            return Ok(None);
        };
        let dto: RepositoryDto = Self::decode("get repository", &response)?;
        Ok(Some(map_repository(dto)))
    }

    /// Looks up a repository by stable backend identifier.
    pub async fn get_repository(&self, id: &RepositoryId) -> ForgeResult<Option<Repository>> {
        let repo = parse_repository_id(id)?;
        self.get_repository_by_path(&RepositoryPath::new(repo.owner, repo.name))
            .await
    }

    /// Lists repositories visible to the client (`GET /user/repos`).
    ///
    /// Forgejo's listing order is not contractual, so the requested sort is
    /// applied client-side with a deterministic owner/name then id tie-break.
    pub async fn list_repositories(&self, query: RepositoryQuery) -> ForgeResult<Vec<Repository>> {
        let dtos: Vec<RepositoryDto> = self
            .list_all("list repositories", "/user/repos", Vec::new())
            .await?;
        let mut repositories: Vec<Repository> = dtos.into_iter().map(map_repository).collect();
        repositories.sort_by(|left, right| compare_repositories(left, right, &query));
        Ok(repositories)
    }

    /// Creates a repository under the current user or an organization.
    ///
    /// The endpoint is `POST /user/repos` when the owner matches the client's
    /// own handle, else `POST /org/{owner}/repos`. `409` maps to
    /// [`ForgeError::AlreadyExists`] and `422` to
    /// [`ForgeError::InvalidRequest`]. The created record is re-fetched by path
    /// so the returned value is normalized through the read mapping.
    pub async fn create_repository(&self, input: CreateRepository) -> ForgeResult<Repository> {
        let current = self.current_user().await?;
        let path = if input.owner == current.handle {
            "/user/repos".to_string()
        } else {
            format!("/org/{}/repos", input.owner)
        };

        let mut body = serde_json::Map::new();
        body.insert("name".to_string(), serde_json::json!(input.name));
        body.insert(
            "default_branch".to_string(),
            serde_json::json!(input.default_branch),
        );
        if let Some(description) = input.description.as_deref().filter(|text| !text.is_empty()) {
            body.insert("description".to_string(), serde_json::json!(description));
        }
        let payload = serde_json::Value::Object(body).to_string();
        let response = self
            .send(HttpMethod::Post, &path, Vec::new(), Some(payload))
            .await?;

        if !response.is_success() {
            // `map_status_error` already maps `422` to `InvalidRequest`; only the
            // create-specific `409 → AlreadyExists` needs an override.
            return Err(match response.status {
                409 => {
                    ForgeError::AlreadyExists(format!("repository {}/{}", input.owner, input.name))
                }
                _ => crate::error::map_status_error("create repository", &response),
            });
        }

        let repo_path = RepositoryPath::new(input.owner.clone(), input.name.clone());
        self.get_repository_by_path(&repo_path)
            .await?
            .ok_or_else(|| {
                ForgeError::Backend(format!(
                    "repository {}/{} was not readable after creation",
                    input.owner, input.name
                ))
            })
    }

    // --- labels ------------------------------------------------------------

    /// Lists repository labels (`GET /repos/{owner}/{repo}/labels`), sorted by
    /// name then id for determinism.
    pub async fn list_labels(&self, repo_id: &RepositoryId) -> ForgeResult<Vec<Label>> {
        let repo = parse_repository_id(repo_id)?;
        let dtos = self.list_label_dtos(&repo).await?;
        let mut labels: Vec<Label> = dtos.into_iter().map(|dto| map_label(&repo, dto)).collect();
        labels.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(labels)
    }

    /// Creates or updates a repository label by name.
    ///
    /// Existing labels are matched by name and patched
    /// (`PATCH …/labels/{id}`); a new name is created (`POST …/labels`).
    /// `color`/`description` are sent when present, and the provider's returned
    /// label object is mapped and returned.
    pub async fn upsert_label(
        &self,
        repo_id: &RepositoryId,
        input: UpsertLabel,
    ) -> ForgeResult<Label> {
        let repo = parse_repository_id(repo_id)?;
        let base = format!("/repos/{}/labels", repo.path_segment());
        let existing = self
            .list_label_dtos(&repo)
            .await?
            .into_iter()
            .find(|label| label.name == input.name);

        let mut body = serde_json::Map::new();
        body.insert("name".to_string(), serde_json::json!(input.name));
        if let Some(color) = input.color.as_deref() {
            body.insert("color".to_string(), serde_json::json!(color));
        }
        if let Some(description) = input.description.as_deref() {
            body.insert("description".to_string(), serde_json::json!(description));
        }
        let payload = serde_json::Value::Object(body).to_string();

        let response = if let Some(existing) = existing {
            let path = format!("{base}/{}", existing.id);
            self.request_checked(
                "update label",
                HttpMethod::Patch,
                &path,
                Vec::new(),
                Some(payload),
            )
            .await?
        } else {
            self.request_checked(
                "create label",
                HttpMethod::Post,
                &base,
                Vec::new(),
                Some(payload),
            )
            .await?
        };
        self.label_ids
            .lock()
            .expect("label id cache mutex poisoned")
            .remove(&repo.path_segment());
        let dto: LabelDto = Self::decode("upsert label", &response)?;
        Ok(map_label(&repo, dto))
    }

    /// Lists the raw label DTOs for a repository (shared by list/upsert).
    async fn list_label_dtos(&self, repo: &RepoCoord) -> ForgeResult<Vec<LabelDto>> {
        let path = format!("/repos/{}/labels", repo.path_segment());
        self.list_all("list labels", &path, Vec::new()).await
    }
}

/// Orders repositories by the requested sort, then owner/name, then id.
fn compare_repositories(
    left: &Repository,
    right: &Repository,
    query: &RepositoryQuery,
) -> Ordering {
    let primary = query
        .sort
        .map(|sort| {
            let comparison = match sort.field {
                RepositorySortField::Path => compare_path(left, right),
                RepositorySortField::CreatedAt => left.created_at.cmp(&right.created_at),
                RepositorySortField::UpdatedAt => left.updated_at.cmp(&right.updated_at),
            };
            match sort.direction {
                SortDirection::Asc => comparison,
                SortDirection::Desc => comparison.reverse(),
            }
        })
        .unwrap_or_else(|| compare_path(left, right));
    primary
        .then_with(|| compare_path(left, right))
        .then_with(|| left.id.cmp(&right.id))
}

/// Deterministic owner-then-name comparison used as the sort tie-break.
fn compare_path(left: &Repository, right: &Repository) -> Ordering {
    left.owner
        .cmp(&right.owner)
        .then_with(|| left.name.cmp(&right.name))
}
