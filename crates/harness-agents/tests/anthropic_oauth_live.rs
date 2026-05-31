//! Live validation for the Anthropic OAuth auth mode.
//!
//! This reads the real shared `~/.pi/agent/auth.json` and uses the `anthropic`
//! entry written by `pi /login anthropic`. It is `#[ignore]`d and gated on
//! `HARNESS_ANTHROPIC_OAUTH=1`, so the default test suite never touches the
//! network or real credentials.
//!
//! ```sh
//! HARNESS_ANTHROPIC_OAUTH=1 \
//!   cargo test -p harness-agents --test anthropic_oauth_live -- --ignored --nocapture
//! ```

use harness_agents::decision::run_decision;
use harness_agents::{AuthChoice, ProviderConfig};
use serde::Deserialize;
use std::time::Instant;

#[derive(Debug, Deserialize)]
struct Pong {
    reply: String,
}

#[test]
#[ignore = "makes real Anthropic OAuth calls; run with HARNESS_ANTHROPIC_OAUTH=1"]
fn anthropic_oauth_validation() {
    if std::env::var("HARNESS_ANTHROPIC_OAUTH").ok().as_deref() != Some("1") {
        eprintln!(
            "skipping Anthropic OAuth live validation: set HARNESS_ANTHROPIC_OAUTH=1 \
             (reads the real ~/.pi/agent/auth.json and makes real Anthropic calls). \
             Run `pi /login anthropic` first."
        );
        return;
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    let provider = ProviderConfig::from_auth(AuthChoice::AnthropicOAuth, None, None)
        .expect("Anthropic OAuth provider builds (run `pi /login anthropic` first)");
    eprintln!("[anthropic] model id: {}", provider.model_id());

    let start = Instant::now();
    let pong: Pong = runtime
        .block_on(run_decision(
            &provider,
            "You reply with a single JSON object and nothing else.",
            r#"Reply with exactly {"reply":"pong"}."#,
        ))
        .expect("Anthropic OAuth smoke decision succeeds and parses");
    assert_eq!(pong.reply.trim().to_lowercase(), "pong");
    eprintln!("[anthropic] smoke decision latency: {:?}", start.elapsed());
}
