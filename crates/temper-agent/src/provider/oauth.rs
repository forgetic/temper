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

/// Env var overriding the auth-file path. The name lives here so the agent's
/// `entry` (the sole env reader) and the worker's env-injection map agree;
/// nothing in this crate reads it.
pub const AUTH_FILE_ENV: &str = "TEMPER_AGENTS_AUTH_FILE";
/// Env var overriding the codex model id. Read only by the agent's `entry`.
pub const CODEX_MODEL_ENV: &str = "TEMPER_AGENTS_CODEX_MODEL";
/// Env var overriding the codex OAuth token endpoint (used by tests). Read only
/// by the agent's `entry`, which forwards the value into [`OAuthSettings`].
pub const CODEX_TOKEN_URL_ENV: &str = "TEMPER_AGENTS_CODEX_TOKEN_URL";

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
    tongs::config::Config::auth_path()
}

/// Resolves the codex model id from the CLI override then the env value.
///
/// Precedence: the `override_id` (the CLI `--codex-model` value), else
/// `env_value` (the [`CODEX_MODEL_ENV`] value the host read), else
/// [`DEFAULT_CODEX_MODEL`]. Reads no environment itself — both inputs are passed.
pub fn resolve_codex_model(override_id: Option<String>, env_value: Option<String>) -> String {
    override_id
        .or(env_value)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_CODEX_MODEL.to_string())
}

/// Where the codex OAuth credential is read from, plus the token endpoint to
/// refresh against.
#[derive(Clone)]
pub struct OAuthSettings {
    auth_file: PathBuf,
    /// The refresh token endpoint; `None` uses the compiled-in [`CODEX_TOKEN_URL`]
    /// (tests override it via [`CODEX_TOKEN_URL_ENV`], resolved in the agent's
    /// `entry`).
    token_url: Option<String>,
}

impl OAuthSettings {
    /// Resolves the auth-file path with the supplied override taking precedence.
    ///
    /// Precedence: the `auth_file` override (the CLI value, with any
    /// [`AUTH_FILE_ENV`] value already folded in by the host), else the SDK
    /// default (`~/.pi/agent/auth.json`). The `token_url` override (the
    /// [`CODEX_TOKEN_URL_ENV`] value the host read) redirects token refresh; `None`
    /// keeps the compiled-in endpoint. Reads no environment.
    pub fn new(auth_file: Option<PathBuf>, token_url: Option<String>) -> Self {
        let auth_file = auth_file
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(tongs::config::Config::auth_path);
        let token_url = token_url
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        Self {
            auth_file,
            token_url,
        }
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
            entry.refresh(self.token_url.as_deref()).await?;
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
    /// stored refresh token, updating the in-memory entry in place. `token_url`
    /// overrides the endpoint when supplied (a test-only redirect resolved in the
    /// agent's `entry`); otherwise the compiled-in [`CODEX_TOKEN_URL`] is used.
    async fn refresh(&mut self, token_url: Option<&str>) -> Result<(), ProviderError> {
        let token_url = token_url.unwrap_or(CODEX_TOKEN_URL);

        let client = tongs::http::client::Client::new();
        let request = client
            .post(token_url)
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
#[path = "oauth_tests.rs"]
mod tests;
