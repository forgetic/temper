// SPDX-License-Identifier: MPL-2.0

//! Process-wide logging init **and** the structured event model for temper.
//!
//! This crate is the single source of temper's logging schema (the
//! [logging & observability design][design]). It owns two things:
//!
//! 1. **The event model** — the vocabulary every emit site draws from:
//!    - [`Service`] — the plane that produced an event (`engine` / `worker` /
//!      `agent` / `trigger`): its `service=` token, its `tracing` target, and its
//!      aligned human prefix.
//!    - [`Event`] — the closed dotted-namespace catalog (`transition.applied`,
//!      `lease.claimed`, …); the machine `event=` key, defined in Rust so it
//!      cannot drift.
//!    - [`WorkItemRef`] — the repo-qualified `artifact.ref` join key
//!      (`acme/widgets#42` / `acme/api PR#19`) shared by the human tag and the
//!      machine field.
//!    - [`format_duration_ms`] / [`format_duration`] — the `1m13s` human
//!      duration renderer (machine fields stay numeric).
//!    - [`redact`] — bounded, secret-scrubbing previews for free-text fields.
//!    - the [`emit`] constructors — one `info`-level site per [`Event`] that
//!      expands to real structured fields plus the generated §7 human line.
//!    - [`work_item_span`] / [`agent_run_span`] — the two span layers that
//!      auto-thread `artifact.ref` onto every child (including debug) line.
//!
//! 2. **The sink wiring** — [`init_logging`], below, unchanged: an
//!    environment-aware global [`tracing`] subscriber (journald under systemd,
//!    ANSI-on-TTY stderr fmt otherwise, `RUST_LOG` filtering).
//!
//! Every temper binary calls [`init_logging`] once at startup to install the
//! global [`tracing`] subscriber. The setup is environment-aware so the same
//! call does the right thing whether the process runs in a terminal, under
//! systemd, or in CI:
//!
//! [design]: https://example.invalid/docs/explanation/logging-and-observability.md
//!
//! - **Level / filtering.** The filter is read from the `RUST_LOG` environment
//!   variable (standard [`EnvFilter`] syntax, e.g. `RUST_LOG=debug` or
//!   `RUST_LOG=temper_engine=trace,info`). When `RUST_LOG` is unset or invalid,
//!   it defaults to `info`.
//! - **journald (systemd).** systemd sets `JOURNAL_STREAM` when the unit's
//!   stderr is connected to the journal. When that variable is present and the
//!   journal socket is reachable, logs go to journald via [`tracing_journald`]
//!   (Linux only). The journal records its own timestamps, so the journald path
//!   stays minimal and does not configure a redundant fmt timer.
//! - **stderr fallback.** Otherwise (no `JOURNAL_STREAM`, the journal socket is
//!   unavailable, or off Linux) logs are written to stderr with a human-readable
//!   fmt layer that keeps timestamps. ANSI colors are enabled only when stderr
//!   is a real TTY ([`IsTerminal`]); under systemd or a pipe, stderr is not a
//!   terminal, so colors are correctly suppressed.
//!
//! [`init_logging`] is idempotent: it installs the subscriber with `try_init`
//! and ignores the "a global default has already been set" error, so calling it
//! more than once (e.g. multiple in-process daemon instances under the sim
//! harness) never panics.
//!
//! [`tracing`]: https://docs.rs/tracing
//! [`tracing_journald`]: https://docs.rs/tracing-journald
//! [`EnvFilter`]: tracing_subscriber::EnvFilter
//! [`IsTerminal`]: std::io::IsTerminal

pub mod duration;
pub mod emit;
pub mod event;
pub mod redact;
pub mod service;
pub mod span;
pub mod work_item;

pub use duration::{format_duration, format_duration_ms};
pub use event::Event;
pub use service::Service;
pub use span::{agent_run_span, work_item_span};
pub use work_item::{ArtifactKind, WorkItemRef, strip_provider_scheme};

use std::io::IsTerminal;

use tracing_subscriber::{EnvFilter, fmt, prelude::*};

/// Build the [`EnvFilter`] from `RUST_LOG`, defaulting to `info`.
///
/// `EnvFilter` is not cheaply cloneable across the two init branches, so each
/// branch calls this to build its own.
fn env_filter() -> EnvFilter {
    filter_from(std::env::var("RUST_LOG").ok().as_deref())
}

/// Pure filter construction: parse `value` (the `RUST_LOG` directive, if any),
/// falling back to `info` when it is absent or fails to parse.
///
/// Factored out from [`env_filter`] so the default/override behavior is
/// unit-testable without mutating the process environment (the workspace forbids
/// `unsafe`, and edition-2024 `set_var` is `unsafe`).
fn filter_from(value: Option<&str>) -> EnvFilter {
    match value {
        Some(directives) => {
            EnvFilter::try_new(directives).unwrap_or_else(|_| EnvFilter::new("info"))
        }
        None => EnvFilter::new("info"),
    }
}

/// Install the global [`tracing`] subscriber for this process.
///
/// See the [crate-level docs](crate) for the env detection rules
/// (`JOURNAL_STREAM`, stderr `is_terminal`, `RUST_LOG` defaulting to `info`).
///
/// Safe to call more than once: it uses `try_init` and swallows the
/// already-initialized error, so a second call is a no-op rather than a panic.
///
/// [`tracing`]: https://docs.rs/tracing
pub fn init_logging() {
    // systemd sets JOURNAL_STREAM when our stderr is the journal. Prefer the
    // journal sink there; fall through to the stderr fmt layer if it is absent
    // or the socket cannot be opened. The journal adds its own timestamps, so
    // this path stays minimal (no fmt timer).
    #[cfg(target_os = "linux")]
    if std::env::var_os("JOURNAL_STREAM").is_some()
        && let Ok(journald) = tracing_journald::layer()
    {
        let _ = tracing_subscriber::registry()
            .with(env_filter())
            .with(journald)
            .try_init();
        return;
    }

    let fmt_layer = fmt::layer()
        .with_ansi(std::io::stderr().is_terminal()) // colors only on a real TTY
        .with_writer(std::io::stderr);

    let _ = tracing_subscriber::registry()
        .with(env_filter())
        .with(fmt_layer)
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `init_logging` installs a GLOBAL subscriber, so the double-call check
    /// must live in a single test to stay deterministic: the second call hits
    /// the already-set global and must be a no-op, not a panic.
    #[test]
    fn init_logging_is_idempotent() {
        init_logging();
        init_logging();
    }

    #[test]
    fn filter_defaults_to_info_when_rust_log_unset() {
        // Same construction init_logging uses, exercised without touching the
        // process environment (the workspace forbids `unsafe`, so we drive the
        // pure helper directly instead of mutating RUST_LOG).
        assert_eq!(filter_from(None).to_string(), "info");
    }

    #[test]
    fn filter_honors_rust_log_override() {
        assert_eq!(filter_from(Some("debug")).to_string(), "debug");
    }

    #[test]
    fn filter_falls_back_to_info_on_invalid_directive() {
        // A malformed directive must not crash init; it falls back to info.
        assert_eq!(
            filter_from(Some("not a valid =@= filter")).to_string(),
            "info"
        );
    }
}
