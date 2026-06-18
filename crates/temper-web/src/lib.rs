//! `temper-web` — the web dashboard for a running temper deployment.
//!
//! This crate is the Rust side of temper-web: it will host the dashboard's own
//! HTTP + SSE server, an in-memory read-model projected from the daemon's three
//! feeds (snapshot poll, log-tail, conversation proxy), and serve the compiled
//! TypeScript UI that lives in `ui/` (bundled to `ui/dist/` by esbuild).
//!
//! See `crates/temper-web/README.md` for the full crate/UI layout and
//! `docs/plans/temper-web-impl-plan.md` for the sub-agent decomposition.
//!
//! # Server note
//!
//! temper-web runs its *own* server rather than reusing the daemon's one-shot
//! HTTP responder (`temper-engine-io/src/http.rs`), which cannot hold a
//! connection open for SSE. The concrete server is built in PR D; this is the
//! foundation skeleton, so it carries no server yet — only a placeholder
//! liveness string so downstream PRs have a stable facade to build on.

/// Liveness placeholder returned by the future `/healthz` endpoint.
///
/// PR D replaces this with a real handler on temper-web's server; for now it
/// exists so the crate compiles, links, and has something to assert in a test.
#[must_use]
pub fn healthz() -> &'static str {
    "ok"
}

#[cfg(test)]
mod tests {
    use super::healthz;

    #[test]
    fn healthz_reports_ok() {
        assert_eq!(healthz(), "ok");
    }
}
