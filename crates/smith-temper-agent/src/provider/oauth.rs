//! ChatGPT (OpenAI Codex) OAuth bearer resolution from the shared auth file.
//!
//! The bearer is the OAuth **access token** (a short-lived JWT whose
//! `chatgpt_account_id` claim the SDK reads in codex mode). It lives in the
//! shared `~/.pi/agent/auth.json` both pi CLIs write. That file has a **dual
//! on-disk schema**: the nodejs pi writes `{ type:"oauth", access, refresh,
//! accountId, expires }`, while the Rust SDK's `AuthCredential::OAuth`
//! (`#[serde(tag="type", rename_all="snake_case")]`) writes `{ type:"o_auth",
//! access_token, refresh_token, expires, token_url?, client_id? }`. We therefore
//! read the file with our **own tolerant reader** that accepts both spellings,
//! and when refreshing a near-expiry token we write the refreshed token back in
//! the **same schema we read** so a nodejs-written file stays nodejs-readable.
//!
//! Secrets discipline: the access/refresh tokens are never logged, formatted, or
//! placed in errors — failures carry only the provider/path and (for refresh) an
//! HTTP status, never token bytes.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{Map, Value};

use super::ProviderError;

/// Env var overriding the auth-file path (used by tests).
pub const AUTH_FILE_ENV: &str = "TEMPER_AGENTS_AUTH_FILE";
/// Env var overriding the codex model id.
pub const CODEX_MODEL_ENV: &str = "TEMPER_AGENTS_CODEX_MODEL";
/// Env var overriding the codex OAuth token endpoint (used by tests).
const TOKEN_URL_ENV: &str = "TEMPER_AGENTS_CODEX_TOKEN_URL";

/// Default codex model the ChatGPT subscription serves (overridable). Listed by
/// the SDK's `openai-codex` route; the route ignores `model.api`.
///
/// Live validation against a real ChatGPT account: the Codex endpoint rejects
/// older `*-codex` ids with "model is not supported when using Codex with a
/// ChatGPT account". Keep this an id the subscription accepts; override via
/// [`CODEX_MODEL_ENV`] when ChatGPT's catalog moves on.
pub const DEFAULT_CODEX_MODEL: &str = "gpt-5.5";
/// Provider key under which the codex credential lives in the auth file.
const PROVIDER_KEY: &str = "openai-codex";
/// Compiled-in OpenAI Codex OAuth token endpoint + public client id (the same
/// constants the SDK's `start/complete_openai_codex_oauth` use).
const CODEX_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const CODEX_OAUTH_SCOPES: &str = "openid profile email";
/// Refresh a token once it is within this many ms of expiry.
const REFRESH_WINDOW_MS: i64 = 5 * 60 * 1000;
/// Safety margin subtracted from a freshly issued token's lifetime.
const EXPIRY_SAFETY_MS: i64 = 5 * 60 * 1000;

/// The default shared auth-file path both pi CLIs write
/// (`~/.pi/agent/auth.json`). Exposed so callers/tests can locate the real file
/// without depending on the SDK directly.
pub fn default_auth_path() -> PathBuf {
    pi::config::Config::auth_path()
}

/// Resolves the configured codex model id (env override or default).
pub fn codex_model_from_env() -> String {
    std::env::var(CODEX_MODEL_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_CODEX_MODEL.to_string())
}

/// Resolves the codex model id with an explicit override taking precedence.
///
/// Precedence: the `override_id` (the CLI value), else [`CODEX_MODEL_ENV`], else
/// [`DEFAULT_CODEX_MODEL`].
pub fn resolve_codex_model(override_id: Option<String>) -> String {
    override_id
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(codex_model_from_env)
}

/// Where the codex OAuth credential is read from.
#[derive(Clone)]
pub struct OAuthSettings {
    auth_file: PathBuf,
}

impl OAuthSettings {
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

    /// Eagerly confirms the codex OAuth entry is present and parseable — without
    /// refreshing — so a missing login fails at setup, before any worker tick.
    pub fn preflight(&self) -> Result<(), ProviderError> {
        CodexEntry::read(&self.auth_file).map(|_| ())
    }

    /// Resolves a fresh access-token bearer, refreshing in place when the stored
    /// token is at or near expiry.
    pub async fn resolve_bearer(&self) -> Result<String, ProviderError> {
        let mut entry = CodexEntry::read(&self.auth_file)?;
        if entry.is_expiring(now_ms()) {
            entry.refresh().await?;
            entry.write_back(&self.auth_file)?;
        }
        Ok(entry.access)
    }
}

impl fmt::Debug for OAuthSettings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OAuthSettings")
            .field("auth_file", &self.auth_file)
            .finish()
    }
}

/// A parsed `openai-codex` OAuth entry plus the schema it was read in.
struct CodexEntry {
    access: String,
    refresh: String,
    expires_ms: i64,
    /// `true` when the entry used the nodejs spelling (`access`/`refresh`).
    nodejs_schema: bool,
    /// The raw entry object, preserved so a write-back keeps unknown fields.
    raw: Map<String, Value>,
}

impl CodexEntry {
    /// Reads and tolerantly parses the codex entry from the auth file.
    fn read(path: &Path) -> Result<Self, ProviderError> {
        let raw = std::fs::read_to_string(path).map_err(|error| {
            ProviderError::OAuthUnavailable(format!(
                "reading {}: {error}; run `pi /login openai-codex` first",
                path.display()
            ))
        })?;
        let root: Value = serde_json::from_str(&raw).map_err(|error| {
            ProviderError::OAuthUnavailable(format!("parsing {}: {error}", path.display()))
        })?;
        let entry = root
            .get(PROVIDER_KEY)
            .and_then(Value::as_object)
            .ok_or_else(|| {
                ProviderError::OAuthUnavailable(format!(
                    "no `{PROVIDER_KEY}` entry in {}; run `pi /login openai-codex` first",
                    path.display()
                ))
            })?;

        let kind = entry
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        // Accept the nodejs (`oauth`) and Rust-SDK (`o_auth`) tags.
        if kind != "oauth" && kind != "o_auth" {
            return Err(ProviderError::OAuthUnavailable(format!(
                "`{PROVIDER_KEY}` entry in {} is `{kind}`, not OAuth; run \
                 `pi /login openai-codex` first",
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

    /// Refreshes the token against the OpenAI Codex token endpoint using the
    /// stored refresh token, updating the in-memory entry in place.
    async fn refresh(&mut self) -> Result<(), ProviderError> {
        let token_url = std::env::var(TOKEN_URL_ENV)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| CODEX_TOKEN_URL.to_string());

        let client = pi::http::client::Client::new();
        let request = client
            .post(&token_url)
            .json(&serde_json::json!({
                "grant_type": "refresh_token",
                "client_id": CODEX_CLIENT_ID,
                "refresh_token": self.refresh,
                "scope": CODEX_OAUTH_SCOPES,
            }))
            .map_err(|error| {
                ProviderError::OAuthUnavailable(format!("building refresh request failed: {error}"))
            })?;

        let response = Box::pin(request.send()).await.map_err(|_| {
            // Never surface the body/error detail — it may echo the refresh token.
            ProviderError::OAuthUnavailable("codex token refresh request failed".to_string())
        })?;
        let status = response.status();
        if !(200..300).contains(&status) {
            return Err(ProviderError::OAuthUnavailable(format!(
                "codex token refresh failed (HTTP {status})"
            )));
        }
        let body = response.text().await.map_err(|_| {
            ProviderError::OAuthUnavailable(
                "reading codex token refresh response failed".to_string(),
            )
        })?;
        let refreshed: RefreshResponse = serde_json::from_str(&body).map_err(|error| {
            ProviderError::OAuthUnavailable(format!(
                "invalid codex token refresh response: {error}"
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
            ProviderError::OAuthUnavailable(format!(
                "auth file {} is not a JSON object",
                path.display()
            ))
        })?;
        object.insert(PROVIDER_KEY.to_string(), Value::Object(self.raw.clone()));
        let serialized = serde_json::to_string_pretty(&root).map_err(|error| {
            ProviderError::OAuthUnavailable(format!("serializing auth file failed: {error}"))
        })?;
        std::fs::write(path, serialized).map_err(|error| {
            ProviderError::OAuthUnavailable(format!("writing {}: {error}", path.display()))
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
    ProviderError::OAuthUnavailable(format!(
        "`{PROVIDER_KEY}` entry in {} is missing its {what}; run `pi /login openai-codex` first",
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
            "smith-temper-agent-oauth-test-{}-{id}.json",
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
}
