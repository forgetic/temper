use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

/// A throwaway auth-file fixture that cleans itself up on drop (the repo
/// avoids the `tempfile` crate; mirror the filesystem-backend test helper).
struct Fixture {
    settings: OAuthSettings,
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
        "anvil-temper-agent-oauth-test-{}-{id}.json",
        std::process::id()
    ));
    std::fs::write(&path, contents).expect("write fixture");
    Fixture {
        settings: OAuthSettings { auth_file: path },
    }
}

#[test]
fn reads_nodejs_schema_access_token() {
    let contents = serde_json::json!({
        "openai-codex": {
            "type": "oauth",
            "access": "node-access-jwt",
            "refresh": "node-refresh",
            "accountId": "acct-123",
            "expires": far_future_ms(),
        }
    })
    .to_string();
    let fixture = write_fixture(&contents);
    let settings = &fixture.settings;
    let entry = CodexEntry::read(&settings.auth_file).expect("read entry");
    assert!(entry.nodejs_schema);
    assert_eq!(entry.access, "node-access-jwt");
    assert!(!entry.is_expiring(now_ms()));
}

#[test]
fn reads_rust_schema_access_token() {
    let contents = serde_json::json!({
        "openai-codex": {
            "type": "o_auth",
            "access_token": "rust-access-jwt",
            "refresh_token": "rust-refresh",
            "expires": far_future_ms(),
        }
    })
    .to_string();
    let fixture = write_fixture(&contents);
    let settings = &fixture.settings;
    let entry = CodexEntry::read(&settings.auth_file).expect("read entry");
    assert!(!entry.nodejs_schema);
    assert_eq!(entry.access, "rust-access-jwt");
}

#[test]
fn missing_entry_is_an_error() {
    let contents = serde_json::json!({ "anthropic": { "type": "oauth" } }).to_string();
    let fixture = write_fixture(&contents);
    let settings = &fixture.settings;
    let Err(error) = CodexEntry::read(&settings.auth_file) else {
        panic!("expected missing entry error");
    };
    assert!(matches!(error, ProviderError::OAuthUnavailable(_)));
    assert!(format!("{error}").contains("openai-codex"));
}

#[test]
fn api_key_only_entry_is_rejected() {
    let contents = serde_json::json!({
        "openai-codex": { "type": "api_key", "key": "sk-secret" }
    })
    .to_string();
    let fixture = write_fixture(&contents);
    let settings = &fixture.settings;
    let Err(error) = CodexEntry::read(&settings.auth_file) else {
        panic!("expected non-oauth error");
    };
    let rendered = format!("{error}");
    assert!(rendered.contains("not OAuth"));
    // The unrelated key bytes never leak into the error.
    assert!(!rendered.contains("sk-secret"));
}

#[test]
fn errors_never_contain_token_bytes() {
    let contents = serde_json::json!({
        "openai-codex": {
            "type": "oauth",
            "access": "super-secret-jwt",
            "expires": far_future_ms(),
        }
    })
    .to_string();
    let fixture = write_fixture(&contents);
    let settings = &fixture.settings;
    let Err(error) = CodexEntry::read(&settings.auth_file) else {
        panic!("expected missing-refresh error");
    };
    let rendered = format!("{error}");
    assert!(rendered.contains("refresh token"));
    assert!(!rendered.contains("super-secret-jwt"));
}

#[test]
fn write_back_preserves_schema_and_other_entries() {
    let contents = serde_json::json!({
        "openai-codex": {
            "type": "oauth",
            "access": "old-access",
            "refresh": "old-refresh",
            "accountId": "acct-123",
            "expires": 0,
        },
        "anthropic": { "type": "oauth", "access": "keep-me" }
    })
    .to_string();
    let fixture = write_fixture(&contents);
    let settings = &fixture.settings;
    let mut entry = CodexEntry::read(&settings.auth_file).expect("read entry");
    entry.access = "new-access".to_string();
    entry.refresh = "new-refresh".to_string();
    entry.expires_ms = far_future_ms();
    entry.sync_raw();
    entry.write_back(&settings.auth_file).expect("write back");

    let reread: Value =
        serde_json::from_str(&std::fs::read_to_string(&settings.auth_file).unwrap()).unwrap();
    let codex = &reread["openai-codex"];
    // nodejs spelling preserved; account id preserved; other entry untouched.
    assert_eq!(codex["access"], "new-access");
    assert_eq!(codex["accountId"], "acct-123");
    assert!(codex.get("access_token").is_none());
    assert_eq!(reread["anthropic"]["access"], "keep-me");
}
