// SPDX-License-Identifier: MPL-2.0

//! Authentication metadata for worker→daemon carriers.
//!
//! The worker token is deliberately *not* part of any serialized worker
//! protocol DTO: split deployments carry it in an HTTP `Authorization: Bearer …`
//! header, while in-process deployments pass this redacted metadata alongside
//! the message. The raw value is exposed only at the transport/authenticator
//! boundary.

use std::fmt;

/// HTTP header used by the split worker protocol transport.
pub const WORKER_AUTHORIZATION_HEADER: &str = "Authorization";
/// Bearer credential scheme used in [`WORKER_AUTHORIZATION_HEADER`].
pub const WORKER_AUTHORIZATION_SCHEME: &str = "Bearer";

/// One worker bearer credential.
#[derive(Clone, Eq, PartialEq)]
pub struct WorkerAuth {
    bearer: String,
}

impl WorkerAuth {
    /// Build a bearer credential from a resolved worker-pool token.
    pub fn bearer(token: impl Into<String>) -> Self {
        Self {
            bearer: token.into(),
        }
    }

    /// Raw bearer token. Use only at I/O/authentication boundaries.
    pub fn expose_bearer(&self) -> &str {
        &self.bearer
    }

    /// Render the HTTP `Authorization` header value.
    pub fn authorization_header_value(&self) -> String {
        format!("{WORKER_AUTHORIZATION_SCHEME} {}", self.bearer)
    }

    /// Parse an HTTP `Authorization` header value, accepting a case-insensitive
    /// `Bearer` scheme and rejecting empty tokens.
    pub fn from_authorization_header(value: &str) -> Option<Self> {
        let mut parts = value.trim().splitn(2, char::is_whitespace);
        let scheme = parts.next()?;
        let token = parts.next()?.trim();
        if !scheme.eq_ignore_ascii_case(WORKER_AUTHORIZATION_SCHEME) {
            return None;
        }
        let token = token.trim();
        if token.is_empty() || token.split_whitespace().count() != 1 {
            return None;
        }
        Some(Self::bearer(token.to_string()))
    }

    /// Constant-interface comparison point for callers. This currently uses
    /// string equality; keeping it behind a method avoids spreading raw-token
    /// comparisons throughout the codebase.
    pub fn matches(&self, presented: &WorkerAuth) -> bool {
        self.bearer == presented.bearer
    }
}

impl fmt::Debug for WorkerAuth {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("WorkerAuth")
            .field(&"[REDACTED]")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bearer_authorization_header() {
        let auth =
            WorkerAuth::from_authorization_header("Bearer pool-token").expect("bearer parses");
        assert_eq!(auth.expose_bearer(), "pool-token");
        assert_eq!(auth.authorization_header_value(), "Bearer pool-token");

        assert_eq!(
            WorkerAuth::from_authorization_header("bearer other")
                .expect("scheme is case insensitive")
                .expose_bearer(),
            "other"
        );
        assert!(WorkerAuth::from_authorization_header("token nope").is_none());
        assert!(WorkerAuth::from_authorization_header("Bearer ").is_none());
        assert!(WorkerAuth::from_authorization_header("Bearer a b").is_none());
    }

    #[test]
    fn debug_redacts_token_value() {
        let rendered = format!("{:?}", WorkerAuth::bearer("super-secret-worker-token"));
        assert!(rendered.contains("[REDACTED]"), "{rendered}");
        assert!(
            !rendered.contains("super-secret-worker-token"),
            "{rendered}"
        );
    }
}
