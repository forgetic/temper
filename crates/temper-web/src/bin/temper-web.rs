// SPDX-License-Identifier: MPL-2.0

//! The `temper-web` server binary — the composition root.
//!
//! This is the ONE place that reads the ambient process environment (argv/env),
//! the sanctioned boundary under the repo's no-ambient-env guard (`src/bin/*`).
//! It snapshots that input into an explicit [`WebConfig`] and threads it into the
//! library, which never reads env itself.
//!
//! Config inputs (flags override env; all optional with standalone defaults):
//!   --bind <addr>            (env TEMPER_WEB_BIND)        default 127.0.0.1:8080
//!   --ui-dir <path>          (env TEMPER_WEB_UI_DIR)      default ./crates/temper-web/ui
//!   --daemon-url <url>       (env TEMPER_WEB_DAEMON_URL)  default none (standalone)
//!   --log-path <path>        (env TEMPER_WEB_LOG_PATH)    default none (no live tail)
//!   --snapshot-fixture <path>                              dev: serve a canned snapshot

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use temper_web::FileLogSource;
use temper_web::config::WebConfig;
use temper_web::feeds::logtail::LogTailAdapter;
use temper_web::feeds::snapshot_source::{
    EmptySnapshotSource, FixtureSnapshotSource, SnapshotSource,
};
use temper_web::project::lanes::LaneMap;
use temper_web::server::{AppState, pump_log_source, serve};

#[path = "temper-web/daemon_client.rs"]
mod daemon_client;
use daemon_client::HttpSnapshotSource;

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
    let source: Box<dyn SnapshotSource> = if let Some(path) = snapshot_fixture {
        let raw = std::fs::read_to_string(&path)
            .map_err(|error| format!("reading snapshot fixture {path}: {error}"))?;
        Box::new(FixtureSnapshotSource::new(raw))
    } else if let Some(url) = config.daemon_url.clone() {
        Box::new(HttpSnapshotSource::new(url))
    } else {
        Box::new(EmptySnapshotSource)
    };

    // No workflow is loaded into temper-web today, so lane derivation runs in its
    // queued/in-flight fallback mode; the log-tail adapter still name-maps
    // lifecycle labels onto lanes. (Wiring a CompiledWorkflow is a follow-up.)
    let lanes = LaneMap::empty();
    let now = now_ms();

    let state = Arc::new(AppState::new(
        source.as_ref(),
        &lanes,
        now,
        config.ui_dir.clone(),
    ));

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

    Ok(WebConfig {
        bind,
        ui_dir,
        daemon_url,
        log_path,
        keep_alive_ms: WebConfig::DEFAULT_KEEP_ALIVE_MS,
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
