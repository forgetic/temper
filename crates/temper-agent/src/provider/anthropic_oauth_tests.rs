use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

use super::super::anthropic_model::DEFAULT_ANTHROPIC_SUBAGENT_MODEL;
use uuid::Uuid;

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    settings: AnthropicOAuthSettings,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.settings.auth_file);
    }
}

fn far_future_ms() -> i64 {
    now_ms() + 60 * 60 * 1000
}

fn write_fixture(contents: &str) -> Fixture {
    let id = NEXT_FIXTURE.fetch_add(1, Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!(
        "anvil-temper-agent-anthropic-oauth-test-{}-{id}.json",
        std::process::id()
    ));
    std::fs::write(&path, contents).expect("write fixture");
    Fixture {
        settings: AnthropicOAuthSettings { auth_file: path },
    }
}

#[test]
fn reads_nodejs_schema_access_token() {
    let contents = serde_json::json!({
        "anthropic": {
            "type": "oauth",
            "access": "sk-ant-oat-node-access",
            "refresh": "node-refresh",
            "expires": far_future_ms(),
        }
    })
    .to_string();
    let fixture = write_fixture(&contents);
    let entry = AnthropicEntry::read(&fixture.settings.auth_file).expect("read entry");
    assert!(entry.nodejs_schema);
    assert_eq!(entry.access, "sk-ant-oat-node-access");
    assert!(!entry.is_expiring(now_ms()));
}

#[test]
fn reads_rust_schema_access_token() {
    let contents = serde_json::json!({
        "anthropic": {
            "type": "o_auth",
            "access_token": "sk-ant-oat-rust-access",
            "refresh_token": "rust-refresh",
            "expires": far_future_ms(),
        }
    })
    .to_string();
    let fixture = write_fixture(&contents);
    let entry = AnthropicEntry::read(&fixture.settings.auth_file).expect("read entry");
    assert!(!entry.nodejs_schema);
    assert_eq!(entry.access, "sk-ant-oat-rust-access");
}

#[test]
fn missing_entry_is_an_error_with_login_hint() {
    let contents = serde_json::json!({ "openai-codex": { "type": "oauth" } }).to_string();
    let fixture = write_fixture(&contents);
    let Err(error) = AnthropicEntry::read(&fixture.settings.auth_file) else {
        panic!("expected missing entry error");
    };
    let rendered = format!("{error}");
    assert!(matches!(error, ProviderError::AnthropicOAuthUnavailable(_)));
    assert!(rendered.contains("anthropic"));
    assert!(rendered.contains("pi /login anthropic"));
}

#[test]
fn write_back_preserves_schema_unknown_fields_and_other_entries() {
    let contents = serde_json::json!({
        "anthropic": {
            "type": "oauth",
            "access": "old-access",
            "refresh": "old-refresh",
            "extra": "keep-me",
            "expires": 0,
        },
        "openai-codex": { "type": "oauth", "access": "keep-codex" }
    })
    .to_string();
    let fixture = write_fixture(&contents);
    let mut entry = AnthropicEntry::read(&fixture.settings.auth_file).expect("read entry");
    entry.access = "new-access".to_string();
    entry.refresh = "new-refresh".to_string();
    entry.expires_ms = far_future_ms();
    entry.sync_raw();
    entry
        .write_back(&fixture.settings.auth_file)
        .expect("write back");

    let reread: Value =
        serde_json::from_str(&std::fs::read_to_string(&fixture.settings.auth_file).unwrap())
            .unwrap();
    let anthropic = &reread["anthropic"];
    assert_eq!(anthropic["access"], "new-access");
    assert_eq!(anthropic["refresh"], "new-refresh");
    assert_eq!(anthropic["extra"], "keep-me");
    assert!(anthropic.get("access_token").is_none());
    assert_eq!(reread["openai-codex"]["access"], "keep-codex");
}

#[test]
fn errors_never_contain_token_bytes() {
    let contents = serde_json::json!({
        "anthropic": {
            "type": "oauth",
            "access": "sk-ant-oat-super-secret",
            "expires": far_future_ms(),
        }
    })
    .to_string();
    let fixture = write_fixture(&contents);
    let Err(error) = AnthropicEntry::read(&fixture.settings.auth_file) else {
        panic!("expected missing-refresh error");
    };
    let rendered = format!("{error}");
    assert!(rendered.contains("refresh token"));
    assert!(!rendered.contains("sk-ant-oat-super-secret"));
}

#[test]
fn request_headers_match_claude_code_identity_without_tokens() {
    let headers = request_headers(DEFAULT_ANTHROPIC_MODEL);
    assert_eq!(
        headers.get("anthropic-version").map(String::as_str),
        Some("2023-06-01")
    );
    assert_eq!(headers.get("x-app").map(String::as_str), Some("cli"));
    assert_eq!(
        headers.get("user-agent").map(String::as_str),
        Some("claude-cli/2.1.139 (external, sdk-cli)")
    );
    let beta = headers.get("anthropic-beta").expect("beta header");
    // Opus (default) gets the full set including the 1M-context beta.
    for flag in [
        "claude-code-20250219",
        "oauth-2025-04-20",
        "context-1m-2025-08-07",
        "effort-2025-11-24",
    ] {
        assert!(beta.contains(flag), "missing beta flag {flag}");
    }
    assert!(Uuid::parse_str(headers.get("x-client-request-id").unwrap()).is_ok());
    assert!(Uuid::parse_str(headers.get("X-Claude-Code-Session-Id").unwrap()).is_ok());
    let rendered = format!("{headers:?}");
    assert!(!rendered.contains("sk-ant"));
    assert!(!rendered.contains("refresh"));
}

#[test]
fn haiku_headers_omit_unentitled_long_context_beta() {
    // The standard subscription rejects the 1M-context beta for Haiku with a
    // 400; the sub-agent tier must not send it. Base flags stay.
    let headers = request_headers(DEFAULT_ANTHROPIC_SUBAGENT_MODEL);
    let beta = headers.get("anthropic-beta").expect("beta header");
    assert!(
        !beta.contains("context-1m"),
        "haiku must not request the long-context beta: {beta}"
    );
    assert!(beta.contains("claude-code-20250219"));
    assert!(beta.contains("effort-2025-11-24"));
    // And its declared limits match what the API serves Haiku on this tier.
    assert_eq!(
        max_output_tokens_for(DEFAULT_ANTHROPIC_SUBAGENT_MODEL),
        64_000
    );
    assert_eq!(
        context_window_for(DEFAULT_ANTHROPIC_SUBAGENT_MODEL),
        200_000
    );
}

#[test]
fn request_headers_use_fresh_ids() {
    let first = request_headers(DEFAULT_ANTHROPIC_MODEL);
    let second = request_headers(DEFAULT_ANTHROPIC_MODEL);
    assert_ne!(
        first.get("x-client-request-id"),
        second.get("x-client-request-id")
    );
    assert_ne!(
        first.get("X-Claude-Code-Session-Id"),
        second.get("X-Claude-Code-Session-Id")
    );
}
