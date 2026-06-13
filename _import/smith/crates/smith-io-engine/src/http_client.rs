//! Outbound HTTP: a pooled skein HTTP/1.1 client for engine executors.
//!
//! Smith's worker is an HTTP *client* of the daemon (it does not run a server),
//! so this module is client-only. An executor builds one [`HttpClient`] and
//! issues [`http_call`]s from inside engine tasks; each call is a request the
//! machine emitted, and its [`HttpResponseData`] becomes a completion.

use std::sync::Arc;

use skein::cx::Cx;
use skein::http::h1::Method;
use skein::http::h1::http_client::{ClientError, HttpClient, HttpClientConfig, RedirectPolicy};

/// One buffered HTTP response, as plain data.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpResponseData {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// One outbound HTTP call, as plain data.
#[derive(Clone, Debug)]
pub struct HttpCall {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Outcome of an [`HttpCall`]: the buffered response, or a transport-error
/// string (no panics — the caller maps it to a completion).
pub type HttpCallResult = Result<HttpResponseData, String>;

/// Build the pooled engine HTTP client (no redirects — the worker protocol
/// treats any redirect as an error, mirroring the previous reqwest config).
pub fn build_http_client() -> Arc<HttpClient> {
    let config = HttpClientConfig {
        redirect_policy: RedirectPolicy::None,
        ..HttpClientConfig::default()
    };
    Arc::new(HttpClient::with_config(config))
}

fn parse_method(method: &str) -> Result<Method, String> {
    match method.to_ascii_uppercase().as_str() {
        "GET" => Ok(Method::Get),
        "POST" => Ok(Method::Post),
        "PUT" => Ok(Method::Put),
        "DELETE" => Ok(Method::Delete),
        "PATCH" => Ok(Method::Patch),
        "HEAD" => Ok(Method::Head),
        "OPTIONS" => Ok(Method::Options),
        other => Err(format!("unsupported HTTP method {other}")),
    }
}

/// Perform one HTTP call on the pooled client from inside an engine task.
pub async fn http_call(cx: &Cx, client: &HttpClient, call: HttpCall) -> HttpCallResult {
    // The 0.2.4-lineage client request is not Cx-threaded; cancellation of the
    // calling task still tears down the in-flight future. Kept in the signature
    // so callers (and a future cancel-aware client) need no change.
    let _ = cx;
    let method = parse_method(&call.method)?;
    let response = client
        .request(method, &call.url, call.headers, call.body)
        .await
        .map_err(|error: ClientError| error.to_string())?;
    Ok(HttpResponseData {
        status: response.status,
        headers: response.headers,
        body: response.body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_method_is_case_insensitive_and_rejects_unknown() {
        assert!(matches!(parse_method("post"), Ok(Method::Post)));
        assert!(matches!(parse_method("GET"), Ok(Method::Get)));
        assert!(parse_method("frobnicate").is_err());
    }
}
