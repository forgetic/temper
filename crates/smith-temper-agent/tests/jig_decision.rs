use std::path::PathBuf;

use jig_core::{Reply, Script};
use jig_server::FakeLlm;
use serde::Deserialize;
use smith_temper_agent::{ProviderConfig, run_decision};

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
fn deepseek_openai_compatible_decision_against_jig() {
    let runtime = runtime();
    let fake = fixed_decision_fake();
    let provider = ProviderConfig::new("deepseek", "deepseek-chat", fake.base_url(), "test-key");

    let decision = run_fixture_decision(&runtime, &provider);

    assert_eq!(decision.action, "advance");
}

#[test]
fn anthropic_oauth_decision_against_jig() {
    let runtime = runtime();
    let fake = fixed_decision_fake();
    let provider = ProviderConfig::anthropic_oauth(Some(jig_auth_fixture()))
        .with_base_url_override(fake.base_url());

    let decision = run_fixture_decision(&runtime, &provider);

    assert_eq!(decision.action, "advance");
}

#[test]
fn chatgpt_oauth_decision_against_jig() {
    let runtime = runtime();
    let fake = fixed_decision_fake();
    let provider = ProviderConfig::chatgpt_oauth(None, Some(jig_auth_fixture()))
        .with_base_url_override(fake.base_url());

    let decision = run_fixture_decision(&runtime, &provider);

    assert_eq!(decision.action, "advance");
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
}

fn fixed_decision_fake() -> FakeLlm {
    FakeLlm::start(Script::Fixed(Reply::text(r#"{"action":"advance"}"#))).expect("start fake LLM")
}

fn run_fixture_decision(
    runtime: &tokio::runtime::Runtime,
    provider: &ProviderConfig,
) -> RoleDecision {
    let role = role_manifest(
        "jig-generic-role-smoke",
        "When the work item is a task in the todo queue with the todo label, choose the advance action.",
    );
    let context = role_context(&role);
    runtime
        .block_on(run_decision::<RoleDecision>(
            provider,
            &role.prompt.render(),
            &context,
        ))
        .expect("jig-backed workflow-role decision succeeds and parses")
}

fn jig_auth_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/jig_auth.json")
}
