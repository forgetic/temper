//! HTTP/provider failure mapping into portable [`ForgeError`] categories.
//!
//! Lookup methods that treat a missing resource as `Ok(None)` should inspect
//! the status before calling [`map_status_error`]; everything else converts a
//! non-success response into the category below.

use crate::client::{HttpError, HttpResponse};
use temper_forge_model::ForgeError;

/// Maximum number of characters of a response body included in error messages.
const SNIPPET_LIMIT: usize = 200;

/// Maps a transport-level failure to [`ForgeError::Backend`].
///
/// Transport errors (DNS, TLS, connection resets) are provider/infrastructure
/// failures, not client mistakes, so they are surfaced as backend errors.
pub(crate) fn map_transport_error(error: HttpError) -> ForgeError {
    ForgeError::Backend(format!("github request failed: {error}"))
}

/// Classifies a non-success HTTP response into a portable [`ForgeError`].
///
/// - `404`/`410` → [`ForgeError::NotFound`] (GitHub serves `410 Gone` for some
///   deleted artifacts; lookups that tolerate absence should convert these to
///   `Ok(None)` before calling this).
/// - `409`/`412` → [`ForgeError::Conflict`] (already-exists / failed
///   precondition such as a stale conditional write).
/// - `400`/`422` → [`ForgeError::InvalidRequest`] (GitHub's validation
///   failures arrive as `422 Unprocessable Entity`).
/// - `401`/`403`/`5xx`/anything else → [`ForgeError::Backend`] (`403` also
///   covers rate limiting, which is an infrastructure outcome, not a caller
///   mistake).
pub(crate) fn map_status_error(context: &str, response: &HttpResponse) -> ForgeError {
    let message = format!(
        "{context}: HTTP {}{}",
        response.status,
        body_snippet(&response.body)
    );
    match response.status {
        404 | 410 => ForgeError::NotFound(message),
        409 | 412 => ForgeError::Conflict(message),
        400 | 422 => ForgeError::InvalidRequest(message),
        _ => ForgeError::Backend(message),
    }
}

/// Returns a `": <snippet>"` suffix for a response body, or an empty string.
fn body_snippet(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let snippet: String = trimmed.chars().take(SNIPPET_LIMIT).collect();
    let ellipsis = if trimmed.chars().count() > SNIPPET_LIMIT {
        "…"
    } else {
        ""
    };
    format!(": {snippet}{ellipsis}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(status: u16, body: &str) -> HttpResponse {
        HttpResponse::new(status, body)
    }

    #[test]
    fn maps_status_codes_to_categories() {
        assert!(matches!(
            map_status_error("lookup", &response(404, "missing")),
            ForgeError::NotFound(_)
        ));
        assert!(matches!(
            map_status_error("lookup", &response(410, "gone")),
            ForgeError::NotFound(_)
        ));
        assert!(matches!(
            map_status_error("create", &response(409, "exists")),
            ForgeError::Conflict(_)
        ));
        assert!(matches!(
            map_status_error("update", &response(412, "stale")),
            ForgeError::Conflict(_)
        ));
        assert!(matches!(
            map_status_error("create", &response(400, "bad")),
            ForgeError::InvalidRequest(_)
        ));
        assert!(matches!(
            map_status_error("create", &response(422, "unprocessable")),
            ForgeError::InvalidRequest(_)
        ));
        assert!(matches!(
            map_status_error("auth", &response(401, "unauthorized")),
            ForgeError::Backend(_)
        ));
        assert!(matches!(
            map_status_error("auth", &response(403, "rate limited")),
            ForgeError::Backend(_)
        ));
        assert!(matches!(
            map_status_error("server", &response(500, "boom")),
            ForgeError::Backend(_)
        ));
    }

    #[test]
    fn includes_body_snippet_in_message() {
        let error = map_status_error("lookup issue", &response(404, "  not found  "));
        assert_eq!(
            error.to_string(),
            "resource not found: lookup issue: HTTP 404: not found"
        );
    }

    #[test]
    fn truncates_long_body_snippet() {
        let body = "x".repeat(SNIPPET_LIMIT + 50);
        let error = map_status_error("create", &response(422, &body));
        let rendered = error.to_string();
        assert!(rendered.ends_with('…'));
        assert!(rendered.contains(&"x".repeat(SNIPPET_LIMIT)));
    }

    #[test]
    fn omits_snippet_for_empty_body() {
        let error = map_status_error("ping", &response(503, "   "));
        assert_eq!(error.to_string(), "backend error: ping: HTTP 503");
    }

    #[test]
    fn transport_failure_is_backend_error() {
        let error = map_transport_error(HttpError::Transport("connection reset".to_string()));
        assert!(matches!(error, ForgeError::Backend(_)));
        assert!(error.to_string().contains("connection reset"));
    }
}
