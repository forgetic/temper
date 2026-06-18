// SPDX-License-Identifier: MPL-2.0

//! The `temper-web` server binary — the composition root.
//!
//! This is the ONE place that reads the ambient process environment (argv/env),
//! the sanctioned boundary under the repo's no-ambient-env guard (`src/bin/*`).
//! It snapshots that input into an explicit [`WebConfig`] and threads it into the
//! library, which never reads env itself.
//!
//! Config inputs (flags override env; all optional with standalone defaults):
//!   --bind <addr>            (env TEMPER_WEB_BIND)             default 127.0.0.1:8080
//!   --ui-dir <path>          (env TEMPER_WEB_UI_DIR)           default ./crates/temper-web/ui
//!   --daemon-url <url>       (env TEMPER_WEB_DAEMON_URL)       default none (standalone)
//!   --log-path <path>        (env TEMPER_WEB_LOG_PATH)         default none (no live tail)
//!   --interaction-url <url>  (env TEMPER_WEB_INTERACTION_URL)  default none (chat proxy off)
//!   --snapshot-poll-ms <ms>  (env TEMPER_WEB_SNAPSHOT_POLL_MS) default none (no re-poll)
//!   --snapshot-fixture <path>                                  dev: serve a canned snapshot

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use temper_web::FileLogSource;
use temper_web::config::WebConfig;
use temper_web::conversation::{ConversationProxy, spawn_poller};
use temper_web::feeds::logtail::LogTailAdapter;
use temper_web::feeds::snapshot_source::{
    EmptySnapshotSource, FixtureSnapshotSource, SnapshotSource,
};
use temper_web::project::lanes::LaneMap;
use temper_web::server::{AppState, pump_log_source, pump_snapshot_source, serve};

#[path = "temper-web/daemon_client.rs"]
mod daemon_client;
use daemon_client::HttpSnapshotSource;

#[path = "temper-web/interaction_client.rs"]
mod interaction_client;
use interaction_client::HttpInteractionClient;

fn main() {
    if let Err(error) = run() {
        eprintln!("temper-web: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let config = parse_config(&args)?;
    let snapshot_fixture = flag(&args, "--snapshot-fixture");

    // Resolve the cold-start snapshot source (explicit; never ambient):
    //   fixture flag > live daemon URL > empty (standalone).
    // `Arc` so the same source seeds cold start AND drives the re-poll pump.
    // `real_source` gates the pump: re-polling an always-empty source is pointless.
    let (source, real_source): (Arc<dyn SnapshotSource>, bool) =
        if let Some(path) = snapshot_fixture {
            let raw = std::fs::read_to_string(&path)
                .map_err(|error| format!("reading snapshot fixture {path}: {error}"))?;
            (Arc::new(FixtureSnapshotSource::new(raw)), true)
        } else if let Some(url) = config.daemon_url.clone() {
            (Arc::new(HttpSnapshotSource::new(url)), true)
        } else {
            (Arc::new(EmptySnapshotSource), false)
        };

    // No workflow is loaded into temper-web today, so lane derivation runs in its
    // queued/in-flight fallback mode; the log-tail adapter still name-maps
    // lifecycle labels onto lanes. (Wiring a CompiledWorkflow is a follow-up.)
    let lanes = LaneMap::empty();
    let now = now_ms();

    let mut state = AppState::new(source.as_ref(), &lanes, now, config.ui_dir.clone());

    // Enable the conversation SSE proxy (feed 2) when an interaction-service URL
    // is configured. The blocking HTTP client is injected here (the binary is the
    // sanctioned env boundary); the library never opens the socket itself.
    let conversation_proxy = config.interaction_url.clone().map(|url| {
        let client = Arc::new(HttpInteractionClient::new(url));
        Arc::new(ConversationProxy::new(client))
    });
    if let Some(proxy) = conversation_proxy.clone() {
        state = state.with_conversation_proxy(proxy);
    }

    let state = Arc::new(state);

    // Drive the conversation poll loop: drain each tracked conversation's events
    // on an interval and fan new ones out as SSE frames (reusing PR D's hub).
    if let Some(proxy) = conversation_proxy {
        spawn_poller(proxy, Duration::from_millis(500));
    }

    // Start the live log-tail feed when a log path is configured.
    if let Some(log_path) = config.log_path.clone() {
        match FileLogSource::open(&log_path, true) {
            Ok(file_source) => {
                let adapter = LogTailAdapter::new(lanes.clone(), now);
                pump_log_source(
                    Arc::clone(&state),
                    adapter,
                    file_source,
                    Duration::from_millis(250),
                    now_ms,
                );
            }
            Err(error) => {
                eprintln!("temper-web: cannot tail {}: {error}", log_path.display());
            }
        }
    }

    // Re-poll the daemon snapshot and reconcile it into the read-model so jobs
    // that go in-flight after startup appear without a restart. Only when a poll
    // interval is configured AND a real (daemon/fixture) source exists — re-polling
    // the empty standalone source would do nothing.
    if let Some(poll_ms) = config.snapshot_poll_ms.filter(|ms| *ms > 0 && real_source) {
        pump_snapshot_source(
            Arc::clone(&state),
            Arc::clone(&source),
            lanes.clone(),
            Duration::from_millis(poll_ms),
            now_ms,
        );
    }

    serve(state, &config).map_err(|error| format!("serve failed: {error}"))
}

fn parse_config(args: &[String]) -> Result<WebConfig, String> {
    let bind = flag(args, "--bind")
        .or_else(|| env("TEMPER_WEB_BIND"))
        .unwrap_or_else(|| "127.0.0.1:8080".to_string());
    let bind = bind
        .parse()
        .map_err(|error| format!("invalid --bind {bind}: {error}"))?;

    let ui_dir = flag(args, "--ui-dir")
        .or_else(|| env("TEMPER_WEB_UI_DIR"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("crates/temper-web/ui"));

    let daemon_url = flag(args, "--daemon-url").or_else(|| env("TEMPER_WEB_DAEMON_URL"));
    let log_path = flag(args, "--log-path")
        .or_else(|| env("TEMPER_WEB_LOG_PATH"))
        .map(PathBuf::from);
    let interaction_url =
        flag(args, "--interaction-url").or_else(|| env("TEMPER_WEB_INTERACTION_URL"));

    let snapshot_poll_ms = flag(args, "--snapshot-poll-ms")
        .or_else(|| env("TEMPER_WEB_SNAPSHOT_POLL_MS"))
        .map(|raw| {
            raw.parse::<u64>()
                .map_err(|error| format!("invalid --snapshot-poll-ms {raw}: {error}"))
        })
        .transpose()?;

    Ok(WebConfig {
        bind,
        ui_dir,
        daemon_url,
        log_path,
        interaction_url,
        keep_alive_ms: WebConfig::DEFAULT_KEEP_ALIVE_MS,
        snapshot_poll_ms,
    })
}

/// Read `--name value` from argv.
fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

/// Read an env var (binary boundary only).
fn env(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

/// Current wall-clock epoch milliseconds.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn parses_snapshot_poll_flag() {
        // An explicit `--snapshot-poll-ms` flag wins over any ambient env, so this
        // is deterministic without touching the process environment.
        let config = parse_config(&args(&["--snapshot-poll-ms", "3000"])).expect("parses");
        assert_eq!(config.snapshot_poll_ms, Some(3000));
    }

    #[test]
    fn rejects_non_numeric_snapshot_poll_flag() {
        let error = parse_config(&args(&["--snapshot-poll-ms", "soon"]))
            .expect_err("non-numeric is rejected");
        assert!(error.contains("--snapshot-poll-ms"), "got: {error}");
    }
}
