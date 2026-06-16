// SPDX-License-Identifier: MPL-2.0

//! Logging behavior under the deterministic sim harness (issue #212).
//!
//! These tests pin the contract that the production `temper_log::init_logging`
//! configures — EnvFilter level semantics, the ANSI-off invariant the
//! piped/journald path relies on, and the structured events the daemon emits
//! (notably the `serving` readiness signal with its `addr` field). They assert
//! that *behavior* rather than calling the global `init_logging`: that installs
//! a single process-wide subscriber, so per-test installs would collide. Each
//! test instead builds an equivalent capturing subscriber (the same
//! `EnvFilter` + a fmt layer with `with_ansi(false)`, writing into a shared
//! in-memory buffer) and installs it for the duration of a closure with
//! [`tracing::subscriber::with_default`] — a thread-local guard that never
//! touches the global default and so cannot collide.
//!
//! Determinism: the lab polls its tasks inline on the calling thread, so a
//! thread-local subscriber captures events emitted from lab tasks. Timestamps
//! are disabled (`without_time`) so assertions never depend on wall or virtual
//! time — only on event presence, level, and structured fields.
//!
//! Serving event, real vs. emitted: the `serving` readiness signal lives in
//! `temper-cli-daemon`'s standalone composition (`standalone/mod.rs:212`),
//! whose `run_async` boots a real provider, agent runner, and forge factory and
//! is not wired to run under `LabRuntime` by this harness. Driving it for real
//! is therefore out of the harness's reach, so we take the fallback the issue
//! allows: emit the identical event — `tracing::info!(addr = %addr,
//! "serving")` — from a task driven under the lab, and assert the captured
//! message is `serving` with the bound address present as a structured `addr`
//! field (not merely baked into the message text).

use std::io::Write;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use temper_sim::Sim;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

const MAX_STEPS: u64 = 1_000_000;

/// Shared in-memory log sink: a `MakeWriter` handing out cloned handles onto one
/// `Arc<Mutex<Vec<u8>>>`, so every event the fmt layer writes lands in the same
/// buffer we can read back after the capture closure returns.
#[derive(Clone, Default)]
struct BufferWriter {
    buf: Arc<Mutex<Vec<u8>>>,
}

impl BufferWriter {
    fn contents(&self) -> String {
        String::from_utf8(self.buf.lock().expect("log buffer").clone())
            .expect("fmt output is valid utf-8")
    }
}

impl Write for BufferWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.buf.lock().expect("log buffer").extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> fmt::MakeWriter<'a> for BufferWriter {
    type Writer = BufferWriter;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Build a capturing subscriber equivalent to the one `init_logging` installs:
/// the same `EnvFilter` (here from an explicit directive) plus a fmt layer with
/// ANSI disabled (mirroring the piped/journald case) writing into `buffer`.
/// Timestamps are dropped so assertions stay time-independent and deterministic.
fn capturing_subscriber(
    directive: &str,
    buffer: BufferWriter,
) -> impl tracing::Subscriber + Send + Sync {
    let fmt_layer = fmt::layer()
        .with_ansi(false)
        .without_time()
        .with_writer(buffer);
    tracing_subscriber::registry()
        .with(EnvFilter::new(directive))
        .with(fmt_layer)
}

/// Run `emit` with a fresh capturing subscriber installed thread-locally for the
/// duration of the closure, and return what was captured.
fn capture(directive: &str, emit: impl FnOnce()) -> String {
    let buffer = BufferWriter::default();
    let subscriber = capturing_subscriber(directive, buffer.clone());
    tracing::subscriber::with_default(subscriber, emit);
    buffer.contents()
}

#[test]
fn info_default_suppresses_debug_events() {
    // The default filter `init_logging` falls back to is `info`; a `debug!`
    // event is therefore below threshold and must not appear in the capture.
    let captured = capture("info", || {
        tracing::debug!(detail = "poll-backstop chatter", "backstop scan");
    });
    assert!(
        captured.is_empty(),
        "debug event must be suppressed at info level, got: {captured:?}"
    );
}

#[test]
fn debug_level_enables_debug_events() {
    // The same event, now at a `debug` filter (e.g. RUST_LOG=debug), must be
    // captured — proving the EnvFilter level threshold is what gates it.
    let captured = capture("debug", || {
        tracing::debug!(detail = "poll-backstop chatter", "backstop scan");
    });
    assert!(
        captured.contains("backstop scan"),
        "debug event must be captured at debug level, got: {captured:?}"
    );
    assert!(
        captured.contains("detail=\"poll-backstop chatter\""),
        "debug event must carry its structured field, got: {captured:?}"
    );
}

#[test]
fn info_event_is_captured_at_info_level() {
    // Sanity that an at-threshold event does pass the info filter (so the
    // suppression test above is testing the level, not a dead subscriber).
    let captured = capture("info", || {
        tracing::info!("daemon ready");
    });
    assert!(
        captured.contains("daemon ready"),
        "info event must be captured at info level, got: {captured:?}"
    );
}

#[test]
fn captured_output_is_ansi_free() {
    // The piped/journald path runs `with_ansi(false)`; the capture mirrors it.
    // No ESC[ (CSI) sequences may appear — color codes would corrupt journald
    // and piped logs.
    let captured = capture("info", || {
        tracing::error!(code = 7, "boom");
        tracing::warn!("careful");
        tracing::info!(addr = "127.0.0.1:8080", "serving");
    });
    assert!(
        !captured.contains('\u{1b}'),
        "captured output must contain no ANSI escape sequences, got: {captured:?}"
    );
    // The events themselves still made it through (guards against an empty-buffer
    // false pass of the ANSI check).
    assert!(captured.contains("boom") && captured.contains("serving"));
}

#[test]
fn serving_readiness_event_carries_addr_field() {
    // The migrated readiness signal is `tracing::info!(addr = %.., "serving")`.
    // Driving the real standalone daemon under LabRuntime is not supported by
    // this harness (its run_async boots a provider/agent/forge factory), so we
    // emit the identical event from a task driven by the lab and assert the
    // contract: message "serving" plus the bound address as a structured `addr`
    // FIELD, not merely substring-baked into the message.
    let addr: SocketAddr = "127.0.0.1:8080".parse().expect("addr parses");

    let captured = {
        let buffer = BufferWriter::default();
        let subscriber = capturing_subscriber("info", buffer.clone());
        tracing::subscriber::with_default(subscriber, || {
            let mut sim = Sim::new(42);
            // Emit through a lab task: the lab polls it inline on this thread, so
            // the thread-local subscriber captures it — deterministically.
            sim.run_scenario(MAX_STEPS, move |_cx| async move {
                tracing::info!(addr = %addr, "serving");
            });
        });
        buffer.contents()
    };

    assert!(
        captured.contains("serving"),
        "the readiness event message must be `serving`, got: {captured:?}"
    );
    // The address is present as a key=value field, not just inside the message.
    assert!(
        captured.contains("addr=127.0.0.1:8080"),
        "the `addr` field must carry the bound address, got: {captured:?}"
    );
}

#[test]
fn structured_fields_survive_as_key_values() {
    // A representative event with multiple typed fields: each must appear as a
    // `key=value` pair in the fmt output (the structured-logging contract), not
    // collapsed into the message string.
    let captured = capture("info", || {
        tracing::info!(
            verdict = "needs_breakdown",
            job_id = "code-7",
            "verdict chosen"
        );
    });
    assert!(
        captured.contains("verdict=\"needs_breakdown\""),
        "string field must render as key=value, got: {captured:?}"
    );
    assert!(
        captured.contains("job_id=\"code-7\""),
        "second field must render as key=value, got: {captured:?}"
    );
    assert!(
        captured.contains("verdict chosen"),
        "the message must still be present, got: {captured:?}"
    );
}
