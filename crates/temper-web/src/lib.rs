// SPDX-License-Identifier: MPL-2.0

//! `temper-web` — the web dashboard for a running temper deployment.
//!
//! This crate is the Rust side of temper-web: its own HTTP + SSE server, an
//! in-memory read-model projected from the daemon's feeds, and the served UI
//! bundle that lives in `ui/` (bundled to `ui/dist/` by esbuild).
//!
//! See `crates/temper-web/README.md` for the crate/UI layout and
//! `docs/plans/temper-web-impl-plan.md` for the sub-agent decomposition.
//!
//! # Module map
//!
//! - [`board`] — the board wire types (the cross-language contract with
//!   `ui/src/model.ts`): the `{t:"snapshot",…}` envelope and the [`board::BoardEvent`]
//!   delta union.
//! - [`readmodel`] — the in-memory, rebuildable board projection (cards,
//!   workers, problems) with a monotonic `seq` cursor.
//! - [`project`] — projection from raw backend data: the daemon snapshot
//!   ([`project::snapshot`]) and lane derivation ([`project::lanes`]).
//! - [`feeds`] — feed adapters: the log-tail parser ([`feeds::logtail`], feed 1b)
//!   and the cold-start snapshot source seam ([`feeds::snapshot_source`], feed 1a).
//! - [`conversation`] — feed 2: the conversation SSE proxy. A poller turns the
//!   interaction-service's poll-based events into a `/conversations/events` SSE
//!   stream and forwards the turn/accept/new POSTs, reusing [`server::sse`]'s hub.
//! - [`server`] — the blocking HTTP + SSE server and its reusable SSE machinery
//!   ([`server::sse`], reused by both the board feed and the conversation proxy).
//! - [`config`] — the explicit [`config::WebConfig`] (no ambient env in lib code).
//!
//! # Server note
//!
//! temper-web runs its *own* server rather than reusing the daemon's one-shot
//! HTTP responder (`temper-engine-io/src/http.rs`), which cannot hold a
//! connection open for SSE. See [`server`] for the rationale (a dependency-light
//! blocking `std::net` server, no skein, no async runtime).

pub mod board;
pub mod config;
pub mod conversation;
pub mod feeds;
pub mod project;
pub mod readmodel;
pub mod server;
pub mod trace;

mod logsource;
pub use logsource::FileLogSource;

/// Liveness string returned by the `/healthz` endpoint.
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
