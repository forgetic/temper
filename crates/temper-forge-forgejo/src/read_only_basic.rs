//! Mutation-proof HTTP Basic authentication for Forgejo inspection.
//!
//! Generated pre-apply bundles have the configured administrator login and
//! password before they have an API token. This client adapter lets those
//! bundles perform REST reads without minting a token: it replaces any
//! token-oriented `Authorization` header prepared by the backend with HTTP
//! Basic credentials, and rejects every non-`GET` request before delegating to
//! the transport.

use crate::{HttpClient, HttpError, HttpMethod, HttpRequest, HttpResponse};
use async_trait::async_trait;
use base64::Engine;

/// Credentials owned by the read-only boundary.
///
/// The encoded header is kept in this private type so callers cannot retrieve
/// it from the adapter. Its `Debug` implementation deliberately reveals no
/// username, password, or encoded header material.
#[derive(Clone)]
struct BasicCredentials {
    authorization: String,
}

impl BasicCredentials {
    fn new(login: impl AsRef<str>, password: impl AsRef<str>) -> Self {
        let encoded = base64::engine::general_purpose::STANDARD.encode(format!(
            "{}:{}",
            login.as_ref(),
            password.as_ref()
        ));
        Self {
            authorization: format!("Basic {encoded}"),
        }
    }
}

impl std::fmt::Debug for BasicCredentials {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("BasicCredentials(<redacted>)")
    }
}

/// An [`HttpClient`] that permits only Basic-authenticated Forgejo REST reads.
///
/// The method check happens before the wrapped client is called, making this a
/// defense-in-depth boundary even when a full [`crate::ForgejoForge`] method is
/// accidentally used by inspection code.
#[derive(Clone)]
pub struct ReadOnlyBasicAuthClient<C> {
    inner: C,
    credentials: BasicCredentials,
}

impl<C> ReadOnlyBasicAuthClient<C> {
    /// Wraps `inner` with read-only HTTP Basic authentication.
    pub fn new(
        inner: C,
        login: impl AsRef<str>,
        password: impl AsRef<str>,
    ) -> ReadOnlyBasicAuthClient<C> {
        Self {
            inner,
            credentials: BasicCredentials::new(login, password),
        }
    }
}

impl<C> std::fmt::Debug for ReadOnlyBasicAuthClient<C> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ReadOnlyBasicAuthClient")
            .field("credentials", &self.credentials)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl<C: HttpClient> HttpClient for ReadOnlyBasicAuthClient<C> {
    async fn execute(&self, mut request: HttpRequest) -> Result<HttpResponse, HttpError> {
        if request.method != HttpMethod::Get {
            return Err(HttpError::ReadOnlyMethod(request.method));
        }

        // The Forgejo backend prepares token-authenticated requests. Remove all
        // authorization values before adding Basic auth so neither an empty nor
        // a stale token header can accompany the configured credentials.
        request
            .headers
            .retain(|(name, _)| !name.eq_ignore_ascii_case("authorization"));
        request.headers.insert(
            0,
            (
                "Authorization".to_string(),
                self.credentials.authorization.clone(),
            ),
        );
        self.inner.execute(request).await
    }
}
