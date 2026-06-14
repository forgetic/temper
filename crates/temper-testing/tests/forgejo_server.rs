//! Phase 1 smoke test for the throwaway Forgejo fixture.
//!
//! `#[ignore]`d, so the default `cargo test` never downloads a binary or opens a
//! socket. No extra environment variable is required; run it with:
//!
//! ```sh
//! cargo test -p temper-testing --test forgejo_server -- --ignored
//! ```
//!
//! The first run downloads the pinned Forgejo binary into `.cache/forgejo/`
//! (checksum-verified); later runs reuse it. Point `TEMPER_FORGEJO_BINARY` at a
//! pre-downloaded binary to skip the download.
//!
//! This test deliberately reaches the server with a **blocking** HTTP client
//! rather than `ForgejoForge`: the backend's `reqwest` calls are async and need
//! a Tokio reactor, which this sync test (and `temper_testing::block_on`) do
//! not provide. Driving `ForgejoForge` against a live server is exercised by
//! later phases under `#[tokio::test]`. Here we only prove the lifecycle.

use std::time::Duration;
use temper_testing::forgejo_server::{ForgejoServer, ForgejoState};

#[test]
#[ignore = "boots a real Forgejo server; run with --ignored"]
fn server_boots_serves_version_and_tears_down() {
    // Booting already polls `/api/v1/version` to readiness; reaching this line
    // proves download → migrate → web → ready all succeeded.
    let cached = ForgejoServer::start_with_state(&ForgejoState::empty("server-smoke"), |_| {
        Ok::<(), String>(())
    })
    .expect("forgejo server boots from declared empty state");
    let server = cached.server;
    let base = server.base_url().to_string();
    assert!(base.starts_with("http://127.0.0.1:"));

    // Independently confirm the API answers, and capture the port so we can
    // assert the process is gone after drop.
    let version_url = format!("{base}/api/v1/version");
    let client = temper_engine_io::http::BlockingJsonClient::new();
    let body = client
        .send("GET", version_url.as_str(), None, None)
        .map(|response| String::from_utf8_lossy(&response.body).into_owned())
        .expect("version endpoint responds");
    assert!(body.contains("version"), "unexpected /version body: {body}");

    // Dropping the server kills the process and removes the data dir; the port
    // should stop answering shortly after.
    drop(server);
    let mut still_up = true;
    for _ in 0..25 {
        if client
            .send("GET", version_url.as_str(), None, None)
            .is_err()
        {
            still_up = false;
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(
        !still_up,
        "server still answered after drop; teardown failed"
    );
}
