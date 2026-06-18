// SPDX-License-Identifier: MPL-2.0

//! Explicit server configuration.
//!
//! Per the repo's no-ambient-env constraint, library code never reads
//! `std::env`. The binary entry point (`src/bin/temper-web.rs`, the sanctioned
//! boundary) snapshots argv/env into a [`WebConfig`] and threads it in; every
//! field here is set explicitly by that composition root.

use std::net::SocketAddr;
use std::path::PathBuf;

/// All configuration the temper-web server needs, constructed at the binary
/// entry from explicit inputs (flags / env snapshot) — never read ambiently.
#[derive(Debug, Clone)]
pub struct WebConfig {
    /// Address the temper-web HTTP+SSE server binds.
    pub bind: SocketAddr,
    /// Directory of the built UI bundle to serve (`index.html`, `chat.html`,
    /// `styles.css`, `dist/board.js`, `dist/chat.js`). Resolved explicitly — the
    /// server never guesses it from the cwd.
    pub ui_dir: PathBuf,
    /// Base URL of the daemon's read API (`GET /v1/state`), if a daemon is
    /// configured. `None` runs the board standalone against an empty/fixture
    /// snapshot (so the server boots and tests pass with no daemon).
    pub daemon_url: Option<String>,
    /// Path to the `temper-log` JSON-lines file to tail for live deltas, if any.
    /// `None` serves the cold-start snapshot without live updates.
    pub log_path: Option<PathBuf>,
    /// Base URL of the interaction-service's REST API (e.g.
    /// `http://127.0.0.1:8077`), if a conversation backend is configured. `Some`
    /// enables the conversation SSE proxy (feed 2); `None` runs the chat surface
    /// standalone — the proxy routes report 503 and the SSE stream emits only
    /// keep-alives (the chat page connects cleanly, just idle).
    pub interaction_url: Option<String>,
    /// Keep-alive cadence for the SSE pulse, in milliseconds (UX §6.3: ~15s).
    pub keep_alive_ms: u64,
}

impl WebConfig {
    /// Default keep-alive cadence (15s) — the UX-specified board pulse interval.
    pub const DEFAULT_KEEP_ALIVE_MS: u64 = 15_000;

    /// A minimal config for a given bind address and UI dir, standalone (no
    /// daemon, no log tail). Entry points add the daemon URL / log path.
    #[must_use]
    pub fn standalone(bind: SocketAddr, ui_dir: PathBuf) -> Self {
        Self {
            bind,
            ui_dir,
            daemon_url: None,
            log_path: None,
            interaction_url: None,
            keep_alive_ms: Self::DEFAULT_KEEP_ALIVE_MS,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standalone_config_has_no_daemon_or_log() {
        let cfg =
            WebConfig::standalone("127.0.0.1:8080".parse().unwrap(), PathBuf::from("/srv/ui"));
        assert!(cfg.daemon_url.is_none());
        assert!(cfg.log_path.is_none());
        assert!(cfg.interaction_url.is_none());
        assert_eq!(cfg.keep_alive_ms, WebConfig::DEFAULT_KEEP_ALIVE_MS);
    }
}
