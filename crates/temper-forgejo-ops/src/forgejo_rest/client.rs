//! Shared transport, error, and response plumbing for the Forgejo REST helpers.
//!
//! Secrets are only sent in headers/basic auth. Errors include a status and a
//! short response-body snippet, never the authorization value.

use base64::Engine;
use serde_json::Value;
use temper_engine_io::http::{HttpCall, http_call};

/// Per-request deadline, matching the previous client-wide configuration.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Debug)]
pub enum RestError {
    Http(String),
    Api {
        what: String,
        status: u16,
        body: String,
    },
    Shape {
        what: String,
        detail: String,
    },
}

impl std::fmt::Display for RestError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RestError::Http(why) => write!(formatter, "forgejo HTTP error: {why}"),
            RestError::Api { what, status, body } => {
                write!(formatter, "forgejo call '{what}' failed ({status}): {body}")
            }
            RestError::Shape { what, detail } => {
                write!(formatter, "forgejo response '{what}' malformed: {detail}")
            }
        }
    }
}

impl std::error::Error for RestError {}

pub type Result<T> = std::result::Result<T, RestError>;

/// Engine-backed HTTP client for the low-level Forgejo helpers.
///
/// Keeps the name (and the `&Client` parameter convention) of the previous
/// reqwest-based client so call sites stay unchanged.
#[derive(Clone)]
pub struct Client {
    /// Clock capability of the task the client was built on; request
    /// deadlines are computed against it.
    cx: skein::cx::Cx,
    inner: std::sync::Arc<skein::http::h1::http_client::HttpClient>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Client").finish_non_exhaustive()
    }
}

pub(crate) enum Auth<'a> {
    Token(&'a str),
    Basic(&'a str, &'a str),
}

/// A fully-buffered response: status plus body text.
pub(crate) struct RestResponse {
    pub(crate) status: u16,
    pub(crate) body: String,
}

impl RestResponse {
    pub(crate) fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

impl Client {
    pub(crate) async fn send(
        &self,
        method: &str,
        url: String,
        auth: Auth<'_>,
        body: Option<&Value>,
    ) -> Result<RestResponse> {
        let mut headers = vec![match auth {
            Auth::Token(token) => ("Authorization".to_string(), format!("token {token}")),
            Auth::Basic(user, password) => {
                let credentials =
                    base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"));
                ("Authorization".to_string(), format!("Basic {credentials}"))
            }
        }];
        if body.is_some() {
            headers.push(("Content-Type".to_string(), "application/json".to_string()));
        }
        let call = HttpCall {
            method: method.to_string(),
            url,
            headers,
            body: body
                .map(|value| value.to_string().into_bytes())
                .unwrap_or_default(),
        };

        let result = match skein::time::timeout(
            temper_engine_io::runtime::timer_now(&self.cx),
            REQUEST_TIMEOUT,
            Box::pin(http_call(&self.inner, call)),
        )
        .await
        {
            Ok(result) => result,
            Err(_elapsed) => Err(format!(
                "request timed out after {}s",
                REQUEST_TIMEOUT.as_secs()
            )),
        };

        let response = result.map_err(RestError::Http)?;
        Ok(RestResponse {
            status: response.status,
            body: String::from_utf8_lossy(&response.body).into_owned(),
        })
    }
}

/// Builds the REST client. The `cx` is the calling task's clock capability;
/// every request deadline is computed against it.
pub fn http_client(cx: skein::cx::Cx) -> Result<Client> {
    Ok(Client {
        cx,
        inner: temper_engine_io::http::build_http_client(),
    })
}

pub(crate) fn json_ok(resp: RestResponse, what: &str) -> Result<Value> {
    if resp.is_success() {
        serde_json::from_str::<Value>(&resp.body).map_err(|err| RestError::Shape {
            what: what.into(),
            detail: err.to_string(),
        })
    } else {
        Err(api_error(resp, what))
    }
}

pub(crate) fn accept_or_conflict(resp: RestResponse, what: &str) -> Result<()> {
    if resp.is_success() {
        return Ok(());
    }
    let code = resp.status;
    let lower = resp.body.to_lowercase();
    let benign = (code == 409 || code == 422)
        && (lower.contains("exist") || lower.contains("already") || lower.contains("member"));
    if benign {
        Ok(())
    } else {
        Err(RestError::Api {
            what: what.into(),
            status: code,
            body: snippet(&resp.body),
        })
    }
}

pub(crate) fn api_error(resp: RestResponse, what: &str) -> RestError {
    RestError::Api {
        what: what.into(),
        status: resp.status,
        body: snippet(&resp.body),
    }
}

fn snippet(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.chars().count() > 300 {
        let head: String = trimmed.chars().take(300).collect();
        format!("{head}…")
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rest_error_display_does_not_assume_secrets() {
        let error = RestError::Api {
            what: "create repo".into(),
            status: 422,
            body: "already exists".into(),
        };
        assert_eq!(
            error.to_string(),
            "forgejo call 'create repo' failed (422): already exists"
        );
    }

    #[test]
    fn accept_or_conflict_tolerates_benign_conflicts() {
        let benign = RestResponse {
            status: 409,
            body: "user already exists".into(),
        };
        assert!(accept_or_conflict(benign, "create user").is_ok());

        let hostile = RestResponse {
            status: 500,
            body: "boom".into(),
        };
        assert!(accept_or_conflict(hostile, "create user").is_err());
    }
}
