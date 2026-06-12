//! Live validation for the Anthropic OAuth auth mode.
//!
//! This reads the real shared `~/.pi/agent/auth.json` and uses the `anthropic`
//! entry written by `pi /login anthropic`. It is `#[ignore]`d and gated on
//! `TEMPER_ANTHROPIC_OAUTH=1`, so the default test suite never touches the
//! network or real credentials.
//!
//! ```sh
//! TEMPER_ANTHROPIC_OAUTH=1 \
//!   cargo test --test anthropic_oauth_live -- --ignored --nocapture
//! ```

use std::time::Instant;

use serde::Deserialize;
use smith_temper_agent::{AuthChoice, ProviderConfig, run_decision};

#[path = "support/workflow_role_fixture.rs"]
mod workflow_role_fixture;
use workflow_role_fixture::{role_context, role_manifest};

#[derive(Debug, Deserialize)]
struct RoleDecision {
    action: String,
    #[allow(dead_code)]
    #[serde(default)]
    reason: String,
}

#[test]
#[ignore = "makes real Anthropic OAuth calls; run with TEMPER_ANTHROPIC_OAUTH=1"]
fn anthropic_oauth_validation() {
    if std::env::var("TEMPER_ANTHROPIC_OAUTH").ok().as_deref() != Some("1") {
        eprintln!(
            "skipping Anthropic OAuth live validation: set TEMPER_ANTHROPIC_OAUTH=1 \
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

    let role = role_manifest(
        "anthropic-generic-role-smoke",
        "When the work item is a task in the todo queue with the todo label, choose the advance action.",
    );
    let context = role_context(&role);
    let start = Instant::now();
    let decision: RoleDecision = runtime
        .block_on(run_decision(&provider, &role.prompt.render(), &context))
        .expect("Anthropic OAuth generic role decision succeeds and parses");
    assert_eq!(decision.action, "advance");
    eprintln!(
        "[anthropic] generic role decision: {decision:?} latency={:?}",
        start.elapsed()
    );
}
