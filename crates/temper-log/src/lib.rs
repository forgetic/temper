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
//! 2. **The sink wiring** — [`init_logging`], below: an environment-aware global
//!    [`tracing`] subscriber (journald under systemd, an explicit JSON fmt layer
//!    under `TEMPER_LOG_FORMAT=json`, ANSI-on-TTY stderr fmt otherwise, `RUST_LOG`
//!    filtering throughout).
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
//!
//! ## Sink precedence
//!
//! Exactly one sink is chosen, in this order:
//!
//! 1. **Explicit JSON (`TEMPER_LOG_FORMAT=json`).** When the operator sets this,
//!    a [`fmt().json()`][json] layer writes structured JSON lines to stderr
//!    **regardless of TTY or systemd**. This is the highest-precedence choice
//!    *on purpose*: it is the machine sink for **non-journald** deployments
//!    (containers, log shippers) that still want field-level JSON without a
//!    journal, and an explicit operator request wins over auto-detection. Span
//!    fields are included (`.with_current_span(true).with_span_list(true)`) so
//!    the `artifact.ref` join key set on the [`work_item_span`] threads onto
//!    every JSON line, exactly as it does for the human projection.
//! 2. **journald (systemd), when JSON is *not* forced.** systemd sets
//!    `JOURNAL_STREAM` when the unit's stderr is connected to the journal. When
//!    that variable identifies the process's current stderr file descriptor and
//!    the journal socket is reachable, logs go to journald via
//!    [`tracing_journald`] (Linux only). Checking the descriptor matters because
//!    child processes can inherit a stale `JOURNAL_STREAM` after redirecting
//!    stderr to a test log file. journald already gives machine output —
//!    `journalctl -o json` exposes every §3 field as a JSON key — so the
//!    explicit `TEMPER_LOG_FORMAT=json` toggle exists for the
//!    *non*-journald case, and journald stays the default under systemd. The
//!    journal records its own timestamps, so this path stays minimal and does
//!    not configure a redundant fmt timer.
//! 3. **stderr human fallback.** Otherwise (no JSON toggle, no `JOURNAL_STREAM`,
//!    the journal socket is unavailable, or off Linux) logs are written to
//!    stderr with a human-readable fmt layer that keeps timestamps. ANSI colors
//!    are enabled only when stderr is a real TTY ([`IsTerminal`]) and `NO_COLOR`
//!    is unset/empty (<https://no-color.org>); under systemd or a pipe stderr is
//!    not a terminal, so colors are correctly suppressed.
//!
//! The format choice is computed by the pure [`select_format`] helper from the
//! `TEMPER_LOG_FORMAT` value, so the toggle is unit-tested without mutating the
//! process environment.
//!
//! ## OpenTelemetry (`otel` and `otel-otlp`, disabled by default)
//!
//! The `otel` feature installs the tracing layer and canonical activity
//! projector. The `otel-otlp` feature additionally installs a batch OTLP/HTTP
//! exporter configured by standard `OTEL_EXPORTER_OTLP_*` variables. Canonical
//! run/scope/turn/model/tool spans are emitted only after durable journal
//! ingestion; exporter failure has no return path into assigned work.
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
//! [json]: tracing_subscriber::fmt::Layer::json
//! [otel]: https://docs.rs/tracing-opentelemetry

pub mod activity;
pub mod duration;
pub mod emit;
pub mod event;
#[cfg(feature = "otel")]
pub mod otel;
pub mod redact;
pub mod service;
pub mod span;
pub mod validation;
pub mod work_item;

pub use duration::{format_duration, format_duration_ms};
pub use event::Event;
pub use service::Service;
pub use span::{agent_run_span, work_item_span};
pub use validation::{VALIDATION_SUMMARY_PREVIEW_LIMIT, validation_summary_preview};
pub use work_item::{ArtifactKind, WorkItemRef, strip_provider_scheme};

use std::io::IsTerminal;

#[cfg(target_os = "linux")]
use std::os::unix::fs::MetadataExt;

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

/// The output format `init_logging` selects from the `TEMPER_LOG_FORMAT` value.
///
/// This only distinguishes the **explicit** JSON request from "decide normally"
/// — the journald-vs-stderr choice for [`LogFormat::Auto`] is environment
/// detection (`JOURNAL_STREAM`, `is_terminal`) done inside [`init_logging`], not
/// something the operator sets here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogFormat {
    /// Default: pick journald under systemd, else the human stderr fmt layer.
    Auto,
    /// `TEMPER_LOG_FORMAT=json`: force a `fmt().json()` layer on stderr, taking
    /// precedence over journald (the operator explicitly asked for JSON; see the
    /// crate-level "Sink precedence" docs).
    Json,
}

/// Pure format selection: map the `TEMPER_LOG_FORMAT` value to a [`LogFormat`].
///
/// Only the exact token `json` (case-insensitive, surrounding whitespace
/// trimmed) selects [`LogFormat::Json`]; everything else — unset, empty, or an
/// unrecognized value — falls back to [`LogFormat::Auto`] so a typo never
/// silently disables logging. Factored out (like [`filter_from`]) so the toggle
/// is unit-testable without mutating the process environment (the workspace
/// forbids `unsafe`, and edition-2024 `set_var` is `unsafe`).
fn select_format(value: Option<&str>) -> LogFormat {
    match value {
        Some(v) if v.trim().eq_ignore_ascii_case("json") => LogFormat::Json,
        _ => LogFormat::Auto,
    }
}

/// Pure ANSI-color decision for the human stderr layer.
///
/// Honors the `NO_COLOR` convention (<https://no-color.org>): when `no_color`
/// is set to any non-empty value, ANSI color is disabled regardless of whether
/// stderr is a TTY. Otherwise color is enabled only when stderr is a real
/// terminal (`is_terminal`). Factored out (like [`select_format`]) so the toggle
/// is unit-testable without mutating the process environment.
fn use_ansi(no_color: Option<&str>, stderr_is_terminal: bool) -> bool {
    match no_color {
        Some(v) if !v.is_empty() => false,
        _ => stderr_is_terminal,
    }
}

/// Install the global [`tracing`] subscriber for this process.
///
/// See the [crate-level docs](crate) for the env detection rules
/// (`JOURNAL_STREAM` matching stderr, stderr `is_terminal`, `RUST_LOG`
/// defaulting to `info`).
///
/// Safe to call more than once: it uses `try_init` and swallows the
/// already-initialized error, so a second call is a no-op rather than a panic.
///
/// [`tracing`]: https://docs.rs/tracing
pub fn init_logging() {
    // Explicit JSON wins over everything (including journald): an operator who
    // sets TEMPER_LOG_FORMAT=json is asking for machine output on a non-journald
    // deployment. See the crate-level "Sink precedence" docs.
    if select_format(std::env::var("TEMPER_LOG_FORMAT").ok().as_deref()) == LogFormat::Json {
        // `with_current_span` + `with_span_list` carry span fields (notably the
        // `artifact.ref` set on the work_item span) onto every JSON line, so the
        // join key threads through the machine projection just as it does the
        // human one.
        let json_layer = fmt::layer()
            .json()
            .with_current_span(true)
            .with_span_list(true)
            .with_writer(std::io::stderr);

        let _ = tracing_subscriber::registry()
            .with(env_filter())
            .with(maybe_otel_layer())
            .with(json_layer)
            .try_init();
        return;
    }

    // systemd sets JOURNAL_STREAM when our stderr is the journal. Prefer the
    // journal sink only while the variable still points at the current stderr;
    // test harnesses often redirect child stderr to files while inheriting the
    // parent's stale JOURNAL_STREAM. Fall through to the stderr fmt layer when
    // it is absent, stale, or the socket cannot be opened. The journal adds its
    // own timestamps, so this path stays minimal (no fmt timer).
    #[cfg(target_os = "linux")]
    if stderr_is_journal_stream() {
        if let Ok(journald) = tracing_journald::layer() {
            let _ = tracing_subscriber::registry()
                .with(env_filter())
                .with(maybe_otel_layer())
                .with(journald)
                .try_init();
            return;
        }
    }

    // Colors only on a real TTY, and never when NO_COLOR is set (no-color.org).
    let ansi = use_ansi(
        std::env::var("NO_COLOR").ok().as_deref(),
        std::io::stderr().is_terminal(),
    );
    let fmt_layer = fmt::layer().with_ansi(ansi).with_writer(std::io::stderr);

    let _ = tracing_subscriber::registry()
        .with(env_filter())
        .with(maybe_otel_layer())
        .with(fmt_layer)
        .try_init();
}

#[cfg(target_os = "linux")]
fn stderr_is_journal_stream() -> bool {
    let Some(value) = std::env::var_os("JOURNAL_STREAM") else {
        return false;
    };
    let Ok(stderr) = std::fs::metadata("/proc/self/fd/2") else {
        return false;
    };

    journal_stream_matches_dev_ino(&value, stderr.dev(), stderr.ino())
}

#[cfg(target_os = "linux")]
fn journal_stream_matches_dev_ino(value: &std::ffi::OsStr, dev: u64, ino: u64) -> bool {
    parse_journal_stream(value) == Some((dev, ino))
}

#[cfg(target_os = "linux")]
fn parse_journal_stream(value: &std::ffi::OsStr) -> Option<(u64, u64)> {
    let value = value.to_str()?;
    let (dev, ino) = value.split_once(':')?;
    let dev = dev.parse().ok()?;
    let ino = ino.parse().ok()?;
    Some((dev, ino))
}

/// The optional OpenTelemetry layer for [`init_logging`] to chain next to the
/// chosen sink.
///
/// Returns `Some(layer)` only under the `otel` feature; otherwise `None`. An
/// `Option<Layer>` is itself a [`Layer`](tracing_subscriber::Layer) (a `None`
/// is a no-op), so the call site chains it uniformly with `.with(...)` whether
/// or not the feature is enabled — keeping the OTel seam out of the default
/// build entirely while leaving exactly one wiring point.
fn maybe_otel_layer<S>() -> Option<impl tracing_subscriber::Layer<S>>
where
    S: tracing::Subscriber + for<'span> tracing_subscriber::registry::LookupSpan<'span>,
{
    #[cfg(feature = "otel")]
    {
        Some(otel::otel_layer())
    }
    #[cfg(not(feature = "otel"))]
    {
        // `None` still needs a concrete `Layer` type to satisfy `impl Layer`;
        // an identity fmt layer that is never constructed serves as the witness.
        None::<tracing_subscriber::fmt::Layer<S>>
    }
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

    #[test]
    fn format_defaults_to_auto_when_toggle_unset() {
        // No TEMPER_LOG_FORMAT -> environment auto-detection (journald/stderr).
        assert_eq!(select_format(None), LogFormat::Auto);
    }

    #[test]
    fn format_selects_json_on_exact_token() {
        assert_eq!(select_format(Some("json")), LogFormat::Json);
    }

    #[test]
    fn format_json_token_is_case_and_whitespace_insensitive() {
        // Operators are forgiving with case/whitespace; the toggle should be too.
        assert_eq!(select_format(Some("JSON")), LogFormat::Json);
        assert_eq!(select_format(Some("Json")), LogFormat::Json);
        assert_eq!(select_format(Some("  json  ")), LogFormat::Json);
    }

    #[test]
    fn format_falls_back_to_auto_on_unknown_or_empty_value() {
        // A typo or unrelated value must NOT silently disable logging — it falls
        // back to Auto so journald/stderr still installs.
        assert_eq!(select_format(Some("")), LogFormat::Auto);
        assert_eq!(select_format(Some("human")), LogFormat::Auto);
        assert_eq!(select_format(Some("jsonl")), LogFormat::Auto);
        assert_eq!(select_format(Some("text")), LogFormat::Auto);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn journal_stream_parser_accepts_systemd_dev_inode_pair() {
        assert_eq!(
            parse_journal_stream(std::ffi::OsStr::new("9:1006296")),
            Some((9, 1_006_296))
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn journal_stream_match_requires_same_dev_and_inode() {
        let stream = std::ffi::OsStr::new("9:1006296");
        assert!(journal_stream_matches_dev_ino(stream, 9, 1_006_296));
        assert!(!journal_stream_matches_dev_ino(stream, 8, 1_006_296));
        assert!(!journal_stream_matches_dev_ino(stream, 9, 1_006_297));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn journal_stream_parser_rejects_malformed_values() {
        assert_eq!(parse_journal_stream(std::ffi::OsStr::new("")), None);
        assert_eq!(parse_journal_stream(std::ffi::OsStr::new("9")), None);
        assert_eq!(parse_journal_stream(std::ffi::OsStr::new("9:")), None);
        assert_eq!(parse_journal_stream(std::ffi::OsStr::new(":100")), None);
        assert_eq!(parse_journal_stream(std::ffi::OsStr::new("9:100:1")), None);
        assert_eq!(parse_journal_stream(std::ffi::OsStr::new("dev:ino")), None);
    }

    #[test]
    fn ansi_follows_tty_when_no_color_unset_or_empty() {
        // No NO_COLOR (or empty) -> color tracks whether stderr is a terminal.
        assert!(use_ansi(None, true));
        assert!(!use_ansi(None, false));
        assert!(use_ansi(Some(""), true));
        assert!(!use_ansi(Some(""), false));
    }

    #[test]
    fn ansi_disabled_when_no_color_set() {
        // Any non-empty NO_COLOR disables color even on a TTY (no-color.org).
        assert!(!use_ansi(Some("1"), true));
        assert!(!use_ansi(Some("anything"), true));
        assert!(!use_ansi(Some("1"), false));
    }
}
