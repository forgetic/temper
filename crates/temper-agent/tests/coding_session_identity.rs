//! Assert session/cache identity on the real native agent's outgoing requests.
//! All provider responses and OAuth credentials are local fixtures.

use std::path::PathBuf;

use jig_core::Dialect;
use temper_agent::{ProviderConfig, WorkspaceContext, run_coding_agent_native};
use temper_protocol_agent::AgentSessionState;

#[path = "support/coding_agent_workspace.rs"]
mod coding_agent_workspace;
#[path = "support/coding_request_oracle.rs"]
mod coding_request_oracle;

use coding_agent_workspace::{REPO_DIR, TempCheckout};
use coding_request_oracle::{CodingRequestOracle, Request};

#[test]
fn codex_cache_identity_survives_turns_and_resumes_and_separates_sessions() {
    let oracle = CodingRequestOracle::start(Dialect::Codex);
    let provider =
        ProviderConfig::chatgpt_oauth(Some("gpt-6-astra".to_string()), Some(jig_auth_fixture()))
            .with_base_url_override(oracle.base_url());

    // Re-create the run and workspace to exercise resumed-session identity,
    // while keeping the fixture credential identical even for unrelated work.
    for session in ["workstream-a", "workstream-a", "workstream-b"] {
        run_session(&provider, Some(session));
    }
    let requests = oracle.requests();
    assert_eq!(requests.len(), 6, "two model turns in each native run");
    for (pair, session) in
        requests
            .chunks_exact(2)
            .zip(["workstream-a", "workstream-a", "workstream-b"])
    {
        for request in pair {
            assert_eq!(request.body["prompt_cache_key"], session);
            assert_eq!(header(request, "session-id"), Some(session));
            assert_eq!(header(request, "x-client-request-id"), Some(session));
            assert_eq!(request.body["model"], "gpt-6-astra");
            assert_eq!(request.body["reasoning"]["effort"], "xhigh");
        }
        assert!(
            pair[1].body["input"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| { item["type"] == "function_call_output" }),
            "second request includes the actual tool result"
        );
    }
    assert_ne!(
        requests[0].body["prompt_cache_key"],
        requests[4].body["prompt_cache_key"]
    );
}

#[test]
fn codex_without_a_durable_session_omits_cache_identity() {
    let oracle = CodingRequestOracle::start(Dialect::Codex);
    let provider = ProviderConfig::chatgpt_oauth(None, Some(jig_auth_fixture()))
        .with_base_url_override(oracle.base_url());
    for session in [None, Some(""), Some("   ")] {
        run_session(&provider, session);
    }
    let requests = oracle.requests();
    assert_eq!(requests.len(), 6);
    for request in &requests {
        assert_no_codex_identity(request);
    }
}

#[test]
fn other_providers_keep_their_existing_session_wire_contract() {
    let anthropic = CodingRequestOracle::start(Dialect::Anthropic);
    let provider = ProviderConfig::anthropic_oauth(Some(jig_auth_fixture()))
        .with_base_url_override(anthropic.base_url());
    run_session(&provider, Some("anthropic-workstream"));
    let requests = anthropic.requests();
    assert_eq!(requests.len(), 2);
    for request in &requests {
        assert_no_codex_identity(request);
        assert_eq!(
            header(request, "x-claude-code-session-id"),
            Some("anthropic-workstream")
        );
    }

    let compatible = CodingRequestOracle::start(Dialect::OpenAi);
    let provider = ProviderConfig::new(
        "deepseek",
        "deepseek-chat",
        compatible.base_url(),
        "test-key",
    );
    run_session(&provider, Some("api-key-workstream"));
    let requests = compatible.requests();
    assert_eq!(requests.len(), 2);
    for request in &requests {
        assert_no_codex_identity(request);
        assert_eq!(header(request, "x-claude-code-session-id"), None);
    }
}

fn run_session(provider: &ProviderConfig, session: Option<&str>) {
    let checkout = TempCheckout::new("coding-session-identity");
    checkout.init_git();
    let mut context: WorkspaceContext = serde_json::from_str(include_str!(
        "../../temper-protocol-agent/fixtures/workspace-context-artifact-context.json"
    ))
    .expect("workspace context fixture");
    context.repos[0].dir = REPO_DIR.to_string();
    context.agent_session = session.map(|session_id| AgentSessionState {
        session_id: session_id.to_string(),
        state: Some(serde_json::json!({"resume_state": "not-a-cache-key"})),
    });
    let provider = provider.clone();
    let cwd = checkout.path().to_path_buf();
    // Explicit empty overlay directory keeps the test independent of user config.
    let overlays = tempfile::tempdir().unwrap();
    let overlay_path = overlays.path().to_path_buf();
    temper_agent_io::block_on_with(move |_cx, handle| async move {
        run_coding_agent_native(handle, &provider, &context, &cwd, 4, Some(&overlay_path)).await
    })
    .expect("native fixture coding session completes");
    assert_eq!(
        std::fs::read_to_string(checkout.repo_path().join("NOTES.md")).unwrap(),
        "project notes\n"
    );
}

fn assert_no_codex_identity(request: &Request) {
    assert!(request.body.get("prompt_cache_key").is_none());
    assert_eq!(header(request, "session-id"), None);
    assert_eq!(header(request, "x-client-request-id"), None);
}

fn header<'a>(request: &'a Request, name: &str) -> Option<&'a str> {
    request.headers.get(name).map(String::as_str)
}

fn jig_auth_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/jig_auth.json")
}
