//! REST Actions fetching for the CI read path: the `runs`/`tasks` endpoints,
//! the pull-request head lookup used for run matching, and tolerant array
//! extraction.

use crate::ids::RepoCoord;
use crate::types::PullRequestDto;
use crate::{ForgejoForge, HttpClient, HttpMethod};
use serde::de::DeserializeOwned;
use serde_json::Value;
use temper_forge_model::{ForgeError, ForgeResult, ItemNumber};

/// Bound on Actions list responses, mirroring the reference TypeScript tooling.
const ACTIONS_LIMIT: &str = "200";

impl<C: HttpClient> ForgejoForge<C> {
    /// Fetches a pull request's head SHA and head ref for run matching.
    ///
    /// Returns `None` when the pull request is absent (`404`). Reuses the
    /// existing [`PullRequestDto`] rather than introducing a CI-only DTO.
    pub(super) async fn fetch_pr_head(
        &self,
        repo: &RepoCoord,
        number: ItemNumber,
    ) -> ForgeResult<Option<(Option<String>, Option<String>)>> {
        let path = format!("/repos/{}/pulls/{}", repo.path_segment(), number.get());
        let Some(response) = self
            .request_optional(
                "get pull request for CI",
                HttpMethod::Get,
                &path,
                Vec::new(),
                None,
            )
            .await?
        else {
            return Ok(None);
        };
        let dto: PullRequestDto = Self::decode("get pull request for CI", &response)?;
        let head = dto.head.unwrap_or_default();
        Ok(Some((
            head.sha.filter(|sha| !sha.is_empty()),
            head.branch.filter(|branch| !branch.is_empty()),
        )))
    }

    /// Fetches an Actions list endpoint and decodes its `workflow_runs` array.
    ///
    /// Treats `403`/`404` as an unavailable backend ([`ForgeError::Backend`]) so
    /// missing Actions support never looks like a passed or failed gate.
    pub(super) async fn fetch_actions_array<T: DeserializeOwned>(
        &self,
        context: &str,
        path: &str,
    ) -> ForgeResult<Vec<T>> {
        self.try_fetch_actions_array(context, path)
            .await?
            .ok_or_else(|| {
                ForgeError::Backend(format!("{context}: Forgejo Actions unavailable over REST"))
            })
    }

    /// Like [`Self::fetch_actions_array`] but reports REST unavailability as
    /// `Ok(None)` so the caller can fall back to the web-UI read path.
    ///
    /// A `403`/`404` (the endpoint is absent, as on Forgejo 7.0.x) yields
    /// `Ok(None)`; any other non-2xx status is still a hard [`ForgeError`].
    pub(super) async fn try_fetch_actions_array<T: DeserializeOwned>(
        &self,
        context: &str,
        path: &str,
    ) -> ForgeResult<Option<Vec<T>>> {
        let query = vec![("limit".to_string(), ACTIONS_LIMIT.to_string())];
        let response = self.send(HttpMethod::Get, path, query, None).await?;
        match response.status {
            200..=299 => {
                extract_array(context, &response.body, &["workflow_runs", "runs", "tasks"])
                    .map(Some)
            }
            403 | 404 => Ok(None),
            other => Err(ForgeError::Backend(format!(
                "{context}: unexpected status {other}"
            ))),
        }
    }
}

pub(super) fn runs_path(repo: &RepoCoord) -> String {
    format!("/repos/{}/actions/runs", repo.path_segment())
}

pub(super) fn tasks_path(repo: &RepoCoord) -> String {
    format!("/repos/{}/actions/tasks", repo.path_segment())
}

/// Tolerantly decodes a JSON array that may be bare or wrapped in an object.
fn extract_array<T: DeserializeOwned>(
    context: &str,
    body: &str,
    keys: &[&str],
) -> ForgeResult<Vec<T>> {
    let trimmed = body.trim_start();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    if trimmed.starts_with('[') {
        return serde_json::from_str::<Vec<T>>(body).map_err(|error| {
            ForgeError::Backend(format!("{context}: failed to decode array: {error}"))
        });
    }
    let value: Value = serde_json::from_str(body).map_err(|error| {
        ForgeError::Backend(format!("{context}: failed to decode response: {error}"))
    })?;
    for key in keys {
        match value.get(*key) {
            None | Some(Value::Null) => continue,
            Some(array) => {
                return serde_json::from_value(array.clone()).map_err(|error| {
                    ForgeError::Backend(format!("{context}: failed to decode `{key}`: {error}"))
                });
            }
        }
    }
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ActionTaskDto;

    #[test]
    fn extract_array_handles_wrapped_bare_and_null() {
        let wrapped: Vec<ActionTaskDto> = extract_array(
            "ctx",
            r#"{"workflow_runs":[{"id":1,"name":"build"}]}"#,
            &["workflow_runs"],
        )
        .unwrap();
        assert_eq!(wrapped.len(), 1);
        let bare: Vec<ActionTaskDto> =
            extract_array("ctx", r#"[{"id":2,"name":"test"}]"#, &["workflow_runs"]).unwrap();
        assert_eq!(bare.len(), 1);
        let null: Vec<ActionTaskDto> =
            extract_array("ctx", r#"{"workflow_runs":null}"#, &["workflow_runs"]).unwrap();
        assert!(null.is_empty());
        let empty: Vec<ActionTaskDto> = extract_array("ctx", "   ", &["workflow_runs"]).unwrap();
        assert!(empty.is_empty());
    }
}
