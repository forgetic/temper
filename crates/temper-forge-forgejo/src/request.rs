//! Shared HTTP request plumbing for the Forgejo backend.
//!
//! These inherent methods on [`ForgejoForge`] send prepared Forgejo API requests
//! through the [`HttpClient`] seam and apply the backend's common policies:
//! transient-error retry, status → [`ForgeError`] mapping, `404`-as-absence, JSON
//! decoding, and pagination. The per-domain modules build on them so the
//! request/response policy lives in one place.

use crate::{ForgejoForge, HttpClient, HttpMethod, HttpResponse, client, error};
use serde::de::DeserializeOwned;
use temper_forge::{ForgeError, ForgeResult};

/// Maximum number of paginated pages a list request will fetch before stopping.
///
/// A safety bound so a misbehaving provider that never returns a short page
/// cannot loop forever. The default page size (50) makes this a generous cap.
const MAX_LIST_PAGES: u32 = 1000;

/// How many times a transient (`5xx`) checked request is retried before the
/// error is surfaced. Small: SQLite write contention clears almost immediately,
/// and a persistent `5xx` should fail fast rather than spin.
const TRANSIENT_RETRY_LIMIT: u32 = 4;

/// Whether an HTTP status is a transient server error worth retrying.
///
/// Only `5xx` qualifies: `4xx` (including `409`/`422`) are client/precondition
/// outcomes the workflow must observe, not retry.
fn is_transient(status: u16) -> bool {
    (500..600).contains(&status)
}

impl<C: HttpClient> ForgejoForge<C> {
    /// Sends a prepared Forgejo API request and returns the raw response.
    ///
    /// The path is relative to the host and excludes the `/api/v1` prefix, which
    /// [`client::build_request`] adds. Transport failures are mapped to
    /// [`ForgeError::Backend`](temper_forge::ForgeError); status interpretation
    /// is the caller's responsibility (see [`crate::error`]).
    pub(crate) async fn send(
        &self,
        method: HttpMethod,
        path: impl AsRef<str>,
        query: Vec<(String, String)>,
        body: Option<String>,
    ) -> ForgeResult<HttpResponse> {
        let request = client::build_request(&self.config().token, method, path, query, body);
        self.http_client()
            .execute(request)
            .await
            .map_err(error::map_transport_error)
    }

    /// Sends a request, mapping any non-success status to a [`ForgeError`].
    ///
    /// Transient server errors (HTTP `5xx`) are retried a bounded number of times
    /// before being surfaced. A real Forgejo under concurrent load (several worker
    /// processes editing artifacts at once) intermittently returns `500` from
    /// SQLite contention on a write; without a retry, a single such blip can
    /// partially apply a multi-call transition (e.g. labels succeed, the assignee
    /// PATCH `500`s) and strand the artifact. The body is re-sent unchanged, so
    /// this is safe for the idempotent label/assignee/state writes the workflow
    /// performs; a `5xx` is treated as "the write did not commit". Non-`5xx`
    /// statuses (including `4xx`) are returned on the first response. Offline
    /// contract tests never return `5xx`, so their request counts are unchanged.
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

    /// Sends a request treating `404` as absence (`Ok(None)`).
    pub(crate) async fn request_optional(
        &self,
        context: &str,
        method: HttpMethod,
        path: impl AsRef<str>,
        query: Vec<(String, String)>,
        body: Option<String>,
    ) -> ForgeResult<Option<HttpResponse>> {
        let response = self.send(method, path, query, body).await?;
        if response.status == 404 {
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
                "{context}: failed to decode forgejo response: {error}"
            ))
        })
    }

    /// Fetches every page of a paginated list endpoint and decodes the items.
    ///
    /// Appends `limit`/`page` query parameters to `base_query` and stops when a
    /// page returns fewer than the configured page size (or [`MAX_LIST_PAGES`]
    /// is reached). Ordering is the caller's responsibility.
    pub(crate) async fn list_all<T: DeserializeOwned>(
        &self,
        context: &str,
        path: &str,
        base_query: Vec<(String, String)>,
    ) -> ForgeResult<Vec<T>> {
        let limit = self.config().page_limit.max(1);
        let mut items = Vec::new();
        let mut page = 1u32;
        loop {
            let mut query = base_query.clone();
            query.push(("limit".to_string(), limit.to_string()));
            query.push(("page".to_string(), page.to_string()));
            let response = self
                .request_checked(context, HttpMethod::Get, path, query, None)
                .await?;
            let batch: Vec<T> = Self::decode(context, &response)?;
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
