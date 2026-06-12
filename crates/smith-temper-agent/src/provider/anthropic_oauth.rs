//! Anthropic OAuth bearer resolution and Claude Code-compatible request headers.
//!
//! The bearer is the Anthropic OAuth access token stored in the shared
//! `~/.pi/agent/auth.json` under provider key `anthropic`. As with ChatGPT
//! Codex OAuth, the two pi CLIs can write different schemas, so this module reads
//! both the nodejs spelling (`type:"oauth"`, `access`, `refresh`) and the Rust
//! SDK spelling (`type:"o_auth"`, `access_token`, `refresh_token`) and writes a
//! refreshed token back with the same field spelling it read.
//!
//! This is intentionally Smith-local: the SDK provider remains unpatched. The
//! Claude Code-compatible identity headers are supplied through
//! `StreamOptions.headers`, which the SDK applies after its own defaults.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{Map, Value};
use uuid::Uuid;

use super::ProviderError;
use super::oauth::AUTH_FILE_ENV;

/// Env var overriding the Anthropic model id.
pub const ANTHROPIC_MODEL_ENV: &str = "TEMPER_AGENTS_ANTHROPIC_MODEL";
/// Env var overriding the Anthropic OAuth token endpoint (used by tests).
const TOKEN_URL_ENV: &str = "TEMPER_AGENTS_ANTHROPIC_TOKEN_URL";

/// Default Anthropic model targeted by the OAuth mode (overridable).
pub const DEFAULT_ANTHROPIC_MODEL: &str = "claude-opus-4-8";
/// Identity line Anthropic's Claude **subscription OAuth** path requires as the
/// first `system` block. Any request whose first system block is not exactly
/// this line is rejected with a generic `429 rate_limit_error`
/// (`{"message":"Error"}`), independent of `anthropic-beta` flags. The pinned
/// SDK sends `system` as a single string and never injects this itself, so the
/// decision adapter sends this identity as the system prompt and folds the role
/// prompt into the user turn. Verified live against `claude-opus-4-8`:
/// identity-only system → 200; role-only, arbitrary, or
/// identity-prefixed-then-appended single string → 429; identity as a separate
/// first array block → 200 (but the SDK cannot send an array `system`).
pub const CLAUDE_CODE_SYSTEM_IDENTITY: &str =
    "You are Claude Code, Anthropic's official CLI for Claude.";
/// Provider key under which the Anthropic credential lives in the auth file.
const PROVIDER_KEY: &str = "anthropic";
/// Compiled-in Anthropic OAuth refresh endpoint + public client id (matching the
/// SDK constants for `pi /login anthropic`).
const ANTHROPIC_TOKEN_URL: &str = "https://console.anthropic.com/v1/oauth/token";
const ANTHROPIC_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
/// Refresh a token once it is within this many ms of expiry.
const REFRESH_WINDOW_MS: i64 = 5 * 60 * 1000;
/// Safety margin subtracted from a freshly issued token's lifetime.
const EXPIRY_SAFETY_MS: i64 = 5 * 60 * 1000;

const ANTHROPIC_BETA: &str = concat!(
    "claude-code-20250219,",
    "oauth-2025-04-20,",
    "interleaved-thinking-2025-05-14,",
    "context-management-2025-06-27,",
    "prompt-caching-scope-2026-01-05,",
    "advisor-tool-2026-03-01,",
    "advanced-tool-use-2025-11-20,",
    "context-1m-2025-08-07,",
    "effort-2025-11-24,",
    "extended-cache-ttl-2025-04-11"
);

/// Resolves the configured Anthropic model id (env override or default).
pub fn anthropic_model_from_env() -> String {
    std::env::var(ANTHROPIC_MODEL_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_ANTHROPIC_MODEL.to_string())
}

/// Where the Anthropic OAuth credential is read from.
#[derive(Clone)]
pub struct AnthropicOAuthSettings {
    auth_file: PathBuf,
}

impl AnthropicOAuthSettings {
    /// Resolves the auth-file path with an explicit override taking precedence.
    ///
    /// Precedence: the `auth_file` override (the CLI value), else
    /// [`AUTH_FILE_ENV`], else the SDK default (`~/.pi/agent/auth.json`).
    pub fn new(auth_file: Option<PathBuf>) -> Self {
        let auth_file = auth_file
            .filter(|path| !path.as_os_str().is_empty())
            .or_else(|| {
                std::env::var(AUTH_FILE_ENV)
                    .ok()
                    .filter(|value| !value.trim().is_empty())
                    .map(PathBuf::from)
            })
            .unwrap_or_else(pi::config::Config::auth_path);
        Self { auth_file }
    }

    /// Eagerly confirms the Anthropic OAuth entry is present and parseable —
    /// without refreshing — so a missing login fails at setup.
    pub fn preflight(&self) -> Result<(), ProviderError> {
        AnthropicEntry::read(&self.auth_file).map(|_| ())
    }

    /// Resolves a fresh access-token bearer, refreshing in place when the stored
    /// token is at or near expiry.
    pub async fn resolve_bearer(&self) -> Result<String, ProviderError> {
        let mut entry = AnthropicEntry::read(&self.auth_file)?;
        if entry.is_expiring(now_ms()) {
            entry.refresh().await?;
            entry.write_back(&self.auth_file)?;
        }
        Ok(entry.access)
    }
}

impl fmt::Debug for AnthropicOAuthSettings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AnthropicOAuthSettings")
            .field("auth_file", &self.auth_file)
            .finish()
    }
}

/// Claude Code-compatible headers injected per Anthropic OAuth request.
pub fn request_headers() -> HashMap<String, String> {
    HashMap::from([
        (
            "x-client-request-id".to_string(),
            Uuid::new_v4().to_string(),
        ),
        ("anthropic-beta".to_string(), ANTHROPIC_BETA.to_string()),
        ("anthropic-version".to_string(), "2023-06-01".to_string()),
        (
            "user-agent".to_string(),
            "claude-cli/2.1.139 (external, sdk-cli)".to_string(),
        ),
        ("x-app".to_string(), "cli".to_string()),
        (
            "X-Claude-Code-Session-Id".to_string(),
            Uuid::new_v4().to_string(),
        ),
        ("X-Stainless-Arch".to_string(), "x64".to_string()),
        ("X-Stainless-Lang".to_string(), "js".to_string()),
        ("X-Stainless-OS".to_string(), "Linux".to_string()),
        (
            "X-Stainless-Package-Version".to_string(),
            "0.93.0".to_string(),
        ),
        ("X-Stainless-Retry-Count".to_string(), "0".to_string()),
        ("X-Stainless-Runtime".to_string(), "node".to_string()),
        (
            "X-Stainless-Runtime-Version".to_string(),
            "v24.3.0".to_string(),
        ),
        ("X-Stainless-Timeout".to_string(), "600".to_string()),
    ])
}

/// A parsed `anthropic` OAuth entry plus the schema it was read in.
struct AnthropicEntry {
    access: String,
    refresh: String,
    expires_ms: i64,
    /// `true` when the entry used the nodejs spelling (`access`/`refresh`).
    nodejs_schema: bool,
    /// The raw entry object, preserved so a write-back keeps unknown fields.
    raw: Map<String, Value>,
}

impl AnthropicEntry {
    /// Reads and tolerantly parses the Anthropic entry from the auth file.
    fn read(path: &Path) -> Result<Self, ProviderError> {
        let raw = std::fs::read_to_string(path).map_err(|error| {
            ProviderError::AnthropicOAuthUnavailable(format!(
                "reading {}: {error}; run `pi /login anthropic` first",
                path.display()
            ))
        })?;
        let root: Value = serde_json::from_str(&raw).map_err(|error| {
            ProviderError::AnthropicOAuthUnavailable(format!("parsing {}: {error}", path.display()))
        })?;
        let entry = root
            .get(PROVIDER_KEY)
            .and_then(Value::as_object)
            .ok_or_else(|| {
                ProviderError::AnthropicOAuthUnavailable(format!(
                    "no `{PROVIDER_KEY}` entry in {}; run `pi /login anthropic` first",
                    path.display()
                ))
            })?;

        let kind = entry
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if kind != "oauth" && kind != "o_auth" {
            return Err(ProviderError::AnthropicOAuthUnavailable(format!(
                "`{PROVIDER_KEY}` entry in {} is `{kind}`, not OAuth; run \
                 `pi /login anthropic` first",
                path.display()
            )));
        }

        let nodejs_schema = entry.contains_key("access");
        let access = string_field(entry, "access", "access_token")
            .ok_or_else(|| missing_field(path, "access token"))?;
        let refresh = string_field(entry, "refresh", "refresh_token")
            .ok_or_else(|| missing_field(path, "refresh token"))?;
        let expires_ms = entry
            .get("expires")
            .and_then(Value::as_i64)
            .ok_or_else(|| missing_field(path, "expiry"))?;

        Ok(Self {
            access,
            refresh,
            expires_ms,
            nodejs_schema,
            raw: entry.clone(),
        })
    }

    /// `true` when the token is at or within [`REFRESH_WINDOW_MS`] of expiry.
    fn is_expiring(&self, now_ms: i64) -> bool {
        self.expires_ms <= now_ms.saturating_add(REFRESH_WINDOW_MS)
    }

    /// Refreshes the token against the Anthropic token endpoint using the stored
    /// refresh token, updating the in-memory entry in place.
    async fn refresh(&mut self) -> Result<(), ProviderError> {
        let token_url = std::env::var(TOKEN_URL_ENV)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| ANTHROPIC_TOKEN_URL.to_string());

        let client = pi::http::client::Client::new();
        let request = client
            .post(&token_url)
            .json(&serde_json::json!({
                "grant_type": "refresh_token",
                "client_id": ANTHROPIC_CLIENT_ID,
                "refresh_token": self.refresh,
            }))
            .map_err(|error| {
                ProviderError::AnthropicOAuthUnavailable(format!(
                    "building refresh request failed: {error}"
                ))
            })?;

        let response = Box::pin(request.send()).await.map_err(|_| {
            // Never surface the body/error detail — it may echo the refresh token.
            ProviderError::AnthropicOAuthUnavailable(
                "anthropic token refresh request failed".to_string(),
            )
        })?;
        let status = response.status();
        if !(200..300).contains(&status) {
            return Err(ProviderError::AnthropicOAuthUnavailable(format!(
                "anthropic token refresh failed (HTTP {status})"
            )));
        }
        let body = response.text().await.map_err(|_| {
            ProviderError::AnthropicOAuthUnavailable(
                "reading anthropic token refresh response failed".to_string(),
            )
        })?;
        let refreshed: RefreshResponse = serde_json::from_str(&body).map_err(|error| {
            ProviderError::AnthropicOAuthUnavailable(format!(
                "invalid anthropic token refresh response: {error}"
            ))
        })?;

        self.access = refreshed.access_token;
        if let Some(refresh) = refreshed.refresh_token {
            self.refresh = refresh;
        }
        self.expires_ms = now_ms()
            .saturating_add(refreshed.expires_in.saturating_mul(1000))
            .saturating_sub(EXPIRY_SAFETY_MS);
        self.sync_raw();
        Ok(())
    }

    /// Mirrors the refreshed fields into `raw` using the original schema's
    /// spelling so the on-disk file stays in the schema it was written in.
    fn sync_raw(&mut self) {
        let (access_key, refresh_key) = if self.nodejs_schema {
            ("access", "refresh")
        } else {
            ("access_token", "refresh_token")
        };
        self.raw
            .insert(access_key.to_string(), Value::String(self.access.clone()));
        self.raw
            .insert(refresh_key.to_string(), Value::String(self.refresh.clone()));
        self.raw
            .insert("expires".to_string(), Value::Number(self.expires_ms.into()));
    }

    /// Writes the (refreshed) entry back into the auth file, preserving every
    /// other provider entry.
    fn write_back(&self, path: &Path) -> Result<(), ProviderError> {
        let mut root = match std::fs::read_to_string(path) {
            Ok(raw) => {
                serde_json::from_str::<Value>(&raw).unwrap_or_else(|_| Value::Object(Map::new()))
            }
            Err(_) => Value::Object(Map::new()),
        };
        let object = root.as_object_mut().ok_or_else(|| {
            ProviderError::AnthropicOAuthUnavailable(format!(
                "auth file {} is not a JSON object",
                path.display()
            ))
        })?;
        object.insert(PROVIDER_KEY.to_string(), Value::Object(self.raw.clone()));
        let serialized = serde_json::to_string_pretty(&root).map_err(|error| {
            ProviderError::AnthropicOAuthUnavailable(format!(
                "serializing auth file failed: {error}"
            ))
        })?;
        std::fs::write(path, serialized).map_err(|error| {
            ProviderError::AnthropicOAuthUnavailable(format!("writing {}: {error}", path.display()))
        })
    }
}

/// The token-endpoint refresh response (subset we consume).
#[derive(Deserialize)]
struct RefreshResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: i64,
}

/// Reads a string field accepting either of two key spellings.
fn string_field(entry: &Map<String, Value>, primary: &str, alternate: &str) -> Option<String> {
    entry
        .get(primary)
        .or_else(|| entry.get(alternate))
        .and_then(Value::as_str)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn missing_field(path: &Path, what: &str) -> ProviderError {
    ProviderError::AnthropicOAuthUnavailable(format!(
        "`{PROVIDER_KEY}` entry in {} is missing its {what}; run `pi /login anthropic` first",
        path.display()
    ))
}

/// Current wall-clock time in unix milliseconds.
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|delta| i64::try_from(delta.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

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
            "smith-temper-agent-anthropic-oauth-test-{}-{id}.json",
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
        let headers = request_headers();
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
    fn request_headers_use_fresh_ids() {
        let first = request_headers();
        let second = request_headers();
        assert_ne!(
            first.get("x-client-request-id"),
            second.get("x-client-request-id")
        );
        assert_ne!(
            first.get("X-Claude-Code-Session-Id"),
            second.get("X-Claude-Code-Session-Id")
        );
    }
}
