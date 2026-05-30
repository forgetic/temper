//! Phase 1 smoke test for the throwaway Forgejo harness.
//!
//! `#[ignore]`d **and** gated behind `HARNESS_FORGEJO_E2E=1`, so the default
//! `cargo test` never downloads a binary or opens a socket — exactly like the
//! `harness-forge-forgejo` live tests. Run it with:
//!
//! ```sh
//! HARNESS_FORGEJO_E2E=1 \
//!   cargo test -p harness-testing --test forgejo_server -- --ignored
//! ```
//!
//! The first run downloads the pinned Forgejo binary into `.cache/forgejo/`
//! (checksum-verified); later runs reuse it. Point `HARNESS_FORGEJO_BINARY` at a
//! pre-downloaded binary to skip the download.
//!
//! This test deliberately reaches the server with a **blocking** HTTP client
//! rather than `ForgejoForge`: the backend's `reqwest` calls are async and need
//! a Tokio reactor, which this sync test (and `harness_testing::block_on`) do
//! not provide. Driving `ForgejoForge` against a live server is exercised by
//! later phases under `#[tokio::test]`. Here we only prove the lifecycle.

use harness_testing::forgejo_server::ForgejoServer;
use std::time::Duration;

/// Returns whether the env opt-in is present; prints a skip note when not.
fn enabled() -> bool {
    if std::env::var("HARNESS_FORGEJO_E2E").ok().as_deref() == Some("1") {
        return true;
    }
    eprintln!(
        "skipping Forgejo e2e smoke test: set HARNESS_FORGEJO_E2E=1 to enable \
         (downloads a pinned Forgejo binary into .cache/ on first run)"
    );
    false
}

#[test]
#[ignore = "boots a real Forgejo server; run with HARNESS_FORGEJO_E2E=1 -- --ignored"]
fn server_boots_serves_version_and_tears_down() {
    if !enabled() {
        return;
    }

    // Booting already polls `/api/v1/version` to readiness; reaching this line
    // proves download → migrate → web → ready all succeeded.
    let server = ForgejoServer::start().expect("forgejo server boots");
    let base = server.base_url().to_string();
    assert!(base.starts_with("http://127.0.0.1:"));

    // Independently confirm the API answers, and capture the port so we can
    // assert the process is gone after drop.
    let version_url = format!("{base}/api/v1/version");
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("blocking client builds");
    let body = client
        .get(&version_url)
        .send()
        .expect("version endpoint responds")
        .text()
        .expect("version body reads");
    assert!(
        body.contains("version"),
        "unexpected /version body: {body}"
    );

    // Dropping the server kills the process and removes the data dir; the port
    // should stop answering shortly after.
    drop(server);
    let mut still_up = true;
    for _ in 0..25 {
        if client.get(&version_url).send().is_err() {
            still_up = false;
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(!still_up, "server still answered after drop; teardown failed");
}
