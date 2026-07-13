//! Shared GitHub request plumbing: send, status interpretation, retry, and
//! pagination.
//!
//! These helpers turn the mockable [`HttpClient`](crate::HttpClient) seam into
//! the small set of operations the per-domain modules ([`crate::issues`],
//! [`crate::pulls`], [`crate::repos`], [`crate::ci`]) build on. They contain no
//! domain knowledge: callers supply the path, query, and body and interpret the
//! decoded result.

use crate::{GitHubForge, HttpClient, HttpMethod, HttpResponse, client, error};
use serde::de::DeserializeOwned;
use temper_forge_model::{ForgeError, ForgeResult};

/// Maximum number of paginated pages a list request will fetch before stopping.
///
/// A safety bound so a misbehaving provider that never returns a short page
/// cannot loop forever. The default page size (50) makes this a generous cap.
const MAX_LIST_PAGES: u32 = 1000;

/// How many times a transient (`5xx`) checked request is retried before the
/// error is surfaced. Small: a persistent `5xx` should fail fast rather than
/// spin, while a single blip must not partially apply a multi-call transition.
const TRANSIENT_RETRY_LIMIT: u32 = 4;

/// Whether an HTTP status is a transient server error worth retrying.
///
/// Only `5xx` qualifies: `4xx` (including `409`/`422`) are client/precondition
/// outcomes the workflow must observe, not retry.
fn is_transient(status: u16) -> bool {
    (500..600).contains(&status)
}

impl<C: HttpClient> GitHubForge<C> {
    /// Sends a prepared GitHub API request and returns the raw response.
    ///
    /// The path is relative to the configured API root (which already contains
    /// any `/api/v3` Enterprise prefix). Transport failures are mapped to
    /// [`ForgeError::Backend`](temper_forge_model::ForgeError); status interpretation
    /// is the caller's responsibility (see [`crate::error`]).
    pub(crate) async fn send(
        &self,
        method: HttpMethod,
        path: impl AsRef<str>,
        query: Vec<(String, String)>,
        body: Option<String>,
    ) -> ForgeResult<HttpResponse> {
        let request = client::build_request(&self.config.token, method, path, query, body);
        self.client
            .execute(request)
            .await
            .map_err(error::map_transport_error)
    }

    /// Sends a request, mapping any non-success status to a [`ForgeError`].
    ///
    /// Transient server errors (HTTP `5xx`) are retried a bounded number of
    /// times before being surfaced; the body is re-sent unchanged, which is
    /// safe for the idempotent label/assignee/state writes the workflow
    /// performs. Non-`5xx` statuses (including `4xx`) are returned on the first
    /// response. Offline contract tests never return `5xx`, so their request
    /// counts are unchanged.
    pub(crate) async fn request_checked(
        &self,
        context: &str,
        method: HttpMethod,
        path: impl AsRef<str>,
        query: Vec<(String, String)>,
        body: Option<String>,
    ) -> ForgeResult<HttpResponse> {
        let path = path.as_ref();
        let mut attempt = 0u32;
        loop {
            let response = self.send(method, path, query.clone(), body.clone()).await?;
            if response.is_success() {
                return Ok(response);
            }
            // Retry only transient server errors, a bounded number of times.
            if is_transient(response.status) && attempt < TRANSIENT_RETRY_LIMIT {
                attempt += 1;
                continue;
            }
            return Err(error::map_status_error(context, &response));
        }
    }

    /// Sends a request treating `404` (and GitHub's `410 Gone`) as absence
    /// (`Ok(None)`).
    pub(crate) async fn request_optional(
        &self,
        context: &str,
        method: HttpMethod,
        path: impl AsRef<str>,
        query: Vec<(String, String)>,
        body: Option<String>,
    ) -> ForgeResult<Option<HttpResponse>> {
        let response = self.send(method, path, query, body).await?;
        if response.status == 404 || response.status == 410 {
            return Ok(None);
        }
        if response.is_success() {
            Ok(Some(response))
        } else {
            Err(error::map_status_error(context, &response))
        }
    }

    /// Decodes a JSON response body into `T`, mapping parse errors to
    /// [`ForgeError::Backend`].
    pub(crate) fn decode<T: DeserializeOwned>(
        context: &str,
        response: &HttpResponse,
    ) -> ForgeResult<T> {
        serde_json::from_str(&response.body).map_err(|error| {
            ForgeError::Backend(format!(
                "{context}: failed to decode github response: {error}"
            ))
        })
    }

    /// Fetches every page of a paginated list endpoint and decodes the items.
    ///
    /// Appends GitHub's `per_page`/`page` query parameters to `base_query` and
    /// stops when a page returns fewer than the configured page size (or
    /// [`MAX_LIST_PAGES`] is reached). Ordering is the caller's responsibility.
    pub(crate) async fn list_all<T: DeserializeOwned>(
        &self,
        context: &str,
        path: &str,
        base_query: Vec<(String, String)>,
    ) -> ForgeResult<Vec<T>> {
        self.list_up_to(context, path, base_query, None).await
    }

    /// Fetches at most `max_items` provider rows, reducing the page size and
    /// stopping pagination as soon as the bound is reached. `Some(0)` performs
    /// no provider request.
    pub(crate) async fn list_up_to<T: DeserializeOwned>(
        &self,
        context: &str,
        path: &str,
        base_query: Vec<(String, String)>,
        max_items: Option<usize>,
    ) -> ForgeResult<Vec<T>> {
        if max_items == Some(0) {
            return Ok(Vec::new());
        }
        let configured_limit = self.config.page_limit.max(1) as usize;
        let page_limit = max_items
            .map(|maximum| configured_limit.min(maximum))
            .unwrap_or(configured_limit);
        let mut items = Vec::new();
        let mut page = 1u32;
        loop {
            let remaining = max_items
                .map(|maximum| maximum.saturating_sub(items.len()))
                .unwrap_or(page_limit);
            if remaining == 0 {
                break;
            }
            let mut query = base_query.clone();
            query.push(("per_page".to_string(), page_limit.to_string()));
            query.push(("page".to_string(), page.to_string()));
            let response = self
                .request_checked(context, HttpMethod::Get, path, query, None)
                .await?;
            let batch: Vec<T> = Self::decode(context, &response)?;
            let batch_len = batch.len();
            items.extend(batch.into_iter().take(remaining));
            if batch_len < page_limit
                || max_items.is_some_and(|maximum| items.len() >= maximum)
                || page >= MAX_LIST_PAGES
            {
                break;
            }
            page += 1;
        }
        Ok(items)
    }

    /// Fetches every page of a wrapped paginated list endpoint.
    ///
    /// GitHub's Actions endpoints wrap their arrays in an envelope object
    /// (`{"total_count": …, "workflow_runs": […]}`); `extract` pulls the page's
    /// items out of the decoded envelope.
    pub(crate) async fn list_all_wrapped<E, T>(
        &self,
        context: &str,
        path: &str,
        base_query: Vec<(String, String)>,
        extract: impl Fn(E) -> Vec<T>,
    ) -> ForgeResult<Vec<T>>
    where
        E: DeserializeOwned,
    {
        let limit = self.config.page_limit.max(1);
        let mut items = Vec::new();
        let mut page = 1u32;
        loop {
            let mut query = base_query.clone();
            query.push(("per_page".to_string(), limit.to_string()));
            query.push(("page".to_string(), page.to_string()));
            let response = self
                .request_checked(context, HttpMethod::Get, path, query, None)
                .await?;
            let envelope: E = Self::decode(context, &response)?;
            let batch = extract(envelope);
            let batch_len = batch.len();
            items.extend(batch);
            if batch_len < limit as usize || page >= MAX_LIST_PAGES {
                break;
            }
            page += 1;
        }
        Ok(items)
    }
}
