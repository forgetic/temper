//! Mockable HTTP seam for the Forgejo backend.
//!
//! The [`HttpClient`] trait isolates every later phase from a concrete HTTP
//! library. The real adapter ([`ReqwestHttpClient`]) talks to a live Forgejo
//! instance; tests drive a mock client that records requests and replays canned
//! responses without touching the network.

use async_trait::async_trait;
use std::collections::VecDeque;
use std::time::Instant;
use thiserror::Error;

/// HTTP method used by a Forgejo request.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl HttpMethod {
    /// Returns the uppercase method token.
    pub fn as_str(self) -> &'static str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
            HttpMethod::Put => "PUT",
            HttpMethod::Patch => "PATCH",
            HttpMethod::Delete => "DELETE",
        }
    }
}

impl std::fmt::Display for HttpMethod {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A backend-prepared HTTP request.
///
/// `path` is the request path relative to the Forgejo host, including the
/// `/api/v1` prefix (see [`build_request`]). The base URL join is the client's
/// responsibility so the mock client can ignore it entirely.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpRequest {
    pub method: HttpMethod,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}

/// Secret-free request metadata retained by an explicitly enabled recorder.
///
/// Header values and query values are deliberately absent. Authentication only
/// records whether a header was present and its recognized scheme, so tokens,
/// cookies, and unrelated headers cannot enter scenario evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpRequestProvenance {
    pub method: HttpMethod,
    pub path: String,
    pub query_keys: Vec<String>,
    pub authentication_present: bool,
    pub authentication_scheme: Option<String>,
    pub accepts_json: bool,
}

/// One bounded snapshot of redacted provider request provenance.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HttpRequestProvenanceSnapshot {
    pub requests: Vec<HttpRequestProvenance>,
    pub dropped: usize,
}

#[derive(Debug)]
pub(crate) struct BoundedRequestProvenance {
    capacity: usize,
    dropped: usize,
    requests: VecDeque<HttpRequestProvenance>,
}

impl BoundedRequestProvenance {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity,
            dropped: 0,
            requests: VecDeque::with_capacity(capacity),
        }
    }

    pub(crate) fn record(&mut self, request: &HttpRequest) {
        if self.capacity == 0 {
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        if self.requests.len() == self.capacity {
            self.requests.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
        self.requests.push_back(redact_request(request));
    }

    pub(crate) fn snapshot(&self) -> HttpRequestProvenanceSnapshot {
        HttpRequestProvenanceSnapshot {
            requests: self.requests.iter().cloned().collect(),
            dropped: self.dropped,
        }
    }
}

fn redact_request(request: &HttpRequest) -> HttpRequestProvenance {
    let authorization = request
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("authorization"));
    let authentication_scheme = authorization.and_then(|(_, value)| {
        let (scheme, _) = value.split_once(char::is_whitespace)?;
        match scheme.to_ascii_lowercase().as_str() {
            "token" | "bearer" | "basic" => Some(scheme.to_ascii_lowercase()),
            _ => Some("other".to_string()),
        }
    });
    let accepts_json = request.headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("accept")
            && value
                .split(',')
                .any(|item| item.trim().eq_ignore_ascii_case("application/json"))
    });
    HttpRequestProvenance {
        method: request.method,
        path: request.path.clone(),
        query_keys: request.query.iter().map(|(key, _)| key.clone()).collect(),
        authentication_present: authorization.is_some(),
        authentication_scheme,
        accepts_json,
    }
}

/// An HTTP response observed from the provider (or replayed by the mock).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl HttpResponse {
    /// Builds a response with no headers; convenient for tests.
    pub fn new(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.into(),
        }
    }

    /// Returns whether the status is in the 2xx success range.
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// Returns the first header value matching `name`, case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// Transport-level failure raised before a complete response is observed.
#[derive(Clone, Debug, Error)]
pub enum HttpError {
    #[error("transport failure: {0}")]
    Transport(String),
    /// A non-GET request was stopped by a read-only client boundary.
    #[error("read-only Forgejo client rejected {0} request")]
    ReadOnlyMethod(HttpMethod),
}

/// Async HTTP seam used by the Forgejo backend.
///
/// Implementations join the request path with their configured base URL, send
/// it, and return the observed status, headers, and body. They must not
/// interpret status codes; status-to-[`ForgeError`](temper_forge_model::ForgeError)
/// mapping lives in [`crate::error`].
#[async_trait]
pub trait HttpClient: Send + Sync {
    /// Executes a prepared request and returns the raw response.
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, HttpError>;
}

/// Builds a Forgejo API request mirroring the working TypeScript `api` helper.
///
/// It prefixes the path with `/api/v1` and always sets the `Authorization`,
/// `Content-Type`, and `Accept` headers, matching the reference integration.
pub(crate) fn build_request(
    token: &str,
    method: HttpMethod,
    path: impl AsRef<str>,
    query: Vec<(String, String)>,
    body: Option<String>,
) -> HttpRequest {
    HttpRequest {
        method,
        path: format!("/api/v1{}", path.as_ref()),
        query,
        headers: vec![
            ("Authorization".to_string(), format!("token {token}")),
            ("Content-Type".to_string(), "application/json".to_string()),
            ("Accept".to_string(), "application/json".to_string()),
        ],
        body,
    }
}

/// Real [`HttpClient`] backed by the engine's pooled skein HTTP client.
///
/// From the logic layer's point of view one `execute` call is a single
/// `<io-event-request>`; this adapter is the imperative-shell executor that
/// performs it. Sending a request requires the skein engine runtime to be
/// driving the future. Offline tests use the mock client instead.
#[derive(Clone)]
pub struct EngineHttpClient {
    base_url: String,
    client: std::sync::Arc<skein::http::h1::http_client::HttpClient>,
}

impl std::fmt::Debug for EngineHttpClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EngineHttpClient")
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

impl EngineHttpClient {
    /// Builds a client targeting `base_url`, stripping any trailing slashes.
    ///
    /// Redirects are not auto-followed. Forgejo API operations interpret the
    /// provider's direct response status at the backend boundary.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            client: temper_engine_io::http::build_http_client(),
        }
    }
}

#[async_trait]
impl HttpClient for EngineHttpClient {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        let method = request.method.as_str();
        let normalized_path = normalize_path(&request.path);
        let operation = format!("{method} {normalized_path}");
        let started = Instant::now();
        let mut url = format!("{}{}", self.base_url, request.path);
        if !request.query.is_empty() {
            url.push('?');
            url.push_str(&temper_engine_io::http::encode_query(&request.query));
        }

        let call = temper_engine_io::http::HttpCall {
            method: method.to_string(),
            url,
            headers: request.headers,
            body: request.body.map(String::into_bytes).unwrap_or_default(),
        };
        let result = temper_engine_io::http::http_call(&self.client, call).await;
        let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        match result {
            Ok(response) => {
                tracing::debug!(
                    target: "temper_forge_forgejo",
                    method = %method,
                    operation = %operation,
                    path = %normalized_path,
                    status = response.status,
                    duration_ms,
                    "Forge HTTP request"
                );
                Ok(HttpResponse {
                    status: response.status,
                    headers: response.headers,
                    body: String::from_utf8_lossy(&response.body).into_owned(),
                })
            }
            Err(error) => {
                tracing::debug!(
                    target: "temper_forge_forgejo",
                    method = %method,
                    operation = %operation,
                    path = %normalized_path,
                    status = 0_u16,
                    duration_ms,
                    error = %error,
                    "Forge HTTP request failed"
                );
                Err(HttpError::Transport(error))
            }
        }
    }
}

fn normalize_path(path: &str) -> String {
    path.split('/')
        .map(|segment| {
            if !segment.is_empty() && segment.bytes().all(|byte| byte.is_ascii_digit()) {
                "{id}"
            } else {
                segment
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_request_prefixes_api_path_and_sets_headers() {
        let request = build_request(
            "secret-token",
            HttpMethod::Get,
            "/repos/acme/widgets/issues",
            vec![("state".to_string(), "open".to_string())],
            None,
        );

        assert_eq!(request.method, HttpMethod::Get);
        assert_eq!(request.path, "/api/v1/repos/acme/widgets/issues");
        assert_eq!(
            request.query,
            vec![("state".to_string(), "open".to_string())]
        );
        assert_eq!(request.body, None);
        // Mirrors the headers the reference TypeScript `api` helper always sends.
        assert_eq!(
            request.headers,
            vec![
                (
                    "Authorization".to_string(),
                    "token secret-token".to_string()
                ),
                ("Content-Type".to_string(), "application/json".to_string()),
                ("Accept".to_string(), "application/json".to_string()),
            ]
        );
    }

    #[test]
    fn build_request_carries_json_body() {
        let request = build_request(
            "t",
            HttpMethod::Post,
            "/repos/acme/widgets/issues",
            Vec::new(),
            Some("{\"title\":\"hi\"}".to_string()),
        );

        assert_eq!(request.method, HttpMethod::Post);
        assert_eq!(request.body.as_deref(), Some("{\"title\":\"hi\"}"));
    }

    #[test]
    fn bounded_provenance_redacts_secrets_and_reports_drops() {
        let mut recorder = BoundedRequestProvenance::new(1);
        let first = build_request(
            "first-secret",
            HttpMethod::Get,
            "/repos/acme/widgets/actions/runs",
            vec![("limit".to_string(), "secret-query-value".to_string())],
            None,
        );
        let second = build_request(
            "second-secret",
            HttpMethod::Get,
            "/repos/acme/widgets/actions/runs/42/jobs",
            Vec::new(),
            None,
        );
        recorder.record(&first);
        recorder.record(&second);

        let snapshot = recorder.snapshot();
        assert_eq!(snapshot.dropped, 1);
        assert_eq!(snapshot.requests.len(), 1);
        let retained = &snapshot.requests[0];
        assert_eq!(retained.authentication_scheme.as_deref(), Some("token"));
        assert!(retained.authentication_present);
        assert!(retained.accepts_json);
        assert!(retained.query_keys.is_empty());
        let debug = format!("{snapshot:?}");
        assert!(!debug.contains("first-secret"));
        assert!(!debug.contains("second-secret"));
        assert!(!debug.contains("secret-query-value"));
    }

    #[test]
    fn response_success_and_case_insensitive_header_lookup() {
        let response = HttpResponse {
            status: 200,
            headers: vec![("ETag".to_string(), "\"abc\"".to_string())],
            body: String::new(),
        };

        assert!(response.is_success());
        assert_eq!(response.header("etag"), Some("\"abc\""));
        assert_eq!(response.header("missing"), None);

        assert!(!HttpResponse::new(404, "nope").is_success());
    }

    #[test]
    fn normalizes_resource_ids_without_erasing_repository_coordinates() {
        assert_eq!(
            normalize_path("/api/v1/repos/acme/widgets/issues/42/labels/7"),
            "/api/v1/repos/acme/widgets/issues/{id}/labels/{id}"
        );
        assert_eq!(
            normalize_path("/api/v1/repos/acme/widgets/issues"),
            "/api/v1/repos/acme/widgets/issues"
        );
    }

    #[test]
    fn method_token_is_uppercase() {
        assert_eq!(HttpMethod::Patch.as_str(), "PATCH");
        assert_eq!(HttpMethod::Delete.as_str(), "DELETE");
    }
}
