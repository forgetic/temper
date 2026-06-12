//! Mockable HTTP seam for the Forgejo backend.
//!
//! The [`HttpClient`] trait isolates every later phase from a concrete HTTP
//! library. The real adapter ([`ReqwestHttpClient`]) talks to a live Forgejo
//! instance; tests drive a mock client that records requests and replays canned
//! responses without touching the network.

use async_trait::async_trait;
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
}

/// Async HTTP seam used by the Forgejo backend.
///
/// Implementations join the request path with their configured base URL, send
/// it, and return the observed status, headers, and body. They must not
/// interpret status codes; status-to-[`ForgeError`](temper_forge::ForgeError)
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
    /// Redirects are **not** auto-followed. The REST API returns terminal
    /// `2xx`/`4xx` directly, while the web-UI CI read path ([`crate::ci_ui`])
    /// follows redirects itself and must observe the raw `3xx` — in particular a
    /// successful login is a `303` to `/`, which an auto-following policy would
    /// silently chase to a `200` homepage and make the login look failed.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            client: temper_io_engine::http::build_http_client(),
        }
    }
}

#[async_trait]
impl HttpClient for EngineHttpClient {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        let mut url = format!("{}{}", self.base_url, request.path);
        if !request.query.is_empty() {
            url.push('?');
            url.push_str(&temper_io_engine::http::encode_query(&request.query));
        }

        let call = temper_io_engine::http::HttpCall {
            method: request.method.as_str().to_string(),
            url,
            headers: request.headers,
            body: request.body.map(String::into_bytes).unwrap_or_default(),
        };
        let cx = temper_io_engine::runtime::ambient_cx();
        let response = temper_io_engine::http::http_call(&cx, &self.client, call)
            .await
            .map_err(HttpError::Transport)?;
        Ok(HttpResponse {
            status: response.status,
            headers: response.headers,
            body: String::from_utf8_lossy(&response.body).into_owned(),
        })
    }
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
    fn method_token_is_uppercase() {
        assert_eq!(HttpMethod::Patch.as_str(), "PATCH");
        assert_eq!(HttpMethod::Delete.as_str(), "DELETE");
    }
}
