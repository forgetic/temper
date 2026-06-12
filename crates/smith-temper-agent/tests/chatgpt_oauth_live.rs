//! Phase A3 live validation: prove the ChatGPT (OpenAI Codex) OAuth path
//! actually talks to the Codex endpoint, refreshes a near-expiry token in place,
//! and drives a generic compiled-role decision — all against the human's real
//! subscription.
//!
//! Per the plan's cost policy, this runs on **ChatGPT OAuth** (a flat
//! subscription, not pay-per-token DeepSeek); **no DeepSeek tokens are spent**.
//!
//! Everything here reads the **real** shared `~/.pi/agent/auth.json` (the
//! `openai-codex` entry must already be present — run `pi /login openai-codex`
//! first). The whole validation is **one** gated `#[test]` so its three steps run
//! sequentially: they share the real auth file, and step 3 deliberately exercises
//! the refresh path, so parallel execution would race the file.
//!
//! `#[ignore]`d **and** gated on `TEMPER_CHATGPT_OAUTH=1`: the default
//! `cargo test` never makes this call.
//!
//! ```sh
//! TEMPER_CHATGPT_OAUTH=1 \
//!   cargo test --test chatgpt_oauth_live -- --ignored --nocapture
//! ```

use std::path::PathBuf;
use std::time::Instant;

use serde::Deserialize;
use serde_json::Value;
use smith_temper_agent::{AuthChoice, ProviderConfig, default_auth_path, run_decision};

#[path = "support/workflow_role_fixture.rs"]
mod workflow_role_fixture;
use workflow_role_fixture::{role_context, role_manifest};

/// The minimal decision shape the trivial smoke prompt asks the model to emit.
#[derive(Debug, Deserialize)]
struct Pong {
    reply: String,
}

#[derive(Debug, Deserialize)]
struct RoleDecision {
    action: String,
    #[allow(dead_code)]
    #[serde(default)]
    reason: String,
}

#[test]
#[ignore = "makes real ChatGPT (OpenAI Codex) OAuth calls; \
            run with TEMPER_CHATGPT_OAUTH=1 -- --ignored --nocapture"]
fn chatgpt_oauth_validation() {
    if std::env::var("TEMPER_CHATGPT_OAUTH").ok().as_deref() != Some("1") {
        eprintln!(
            "skipping ChatGPT OAuth live validation: set TEMPER_CHATGPT_OAUTH=1 (reads the real \
             ~/.pi/agent/auth.json and makes real Codex calls). Run `pi /login openai-codex` first."
        );
        return;
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    // Step 1 — end-to-end proof: build the provider against the *real* auth file
    // (default path, no override) and run one trivial decision. `from_auth` runs
    // the credential preflight, so a missing login fails here with a clear error.
    let provider = ProviderConfig::from_auth(AuthChoice::ChatGptOAuth, None, None)
        .expect("ChatGPT OAuth provider builds (run `pi /login openai-codex` first)");
    eprintln!("[a3] codex model id: {}", provider.model_id());

    let smoke_start = Instant::now();
    let pong: Pong = runtime
        .block_on(run_decision(
            &provider,
            "You reply with a single JSON object and nothing else.",
            r#"Reply with exactly {"reply":"pong"}."#,
        ))
        .expect("ChatGPT OAuth smoke decision succeeds and parses");
    let smoke_latency = smoke_start.elapsed();
    assert_eq!(pong.reply.trim().to_lowercase(), "pong");
    eprintln!("[a3] smoke decision latency: {smoke_latency:?}");

    // Step 2 — a real generic workflow-role decision: drive a compiled fixture
    // role prompt through Smith's one-turn decision parser. This proves the
    // OAuth path handles user-defined role prompts without importing any
    // checked-in reference-delivery prompt constant.
    let role = role_manifest(
        "oauth-generic-role-smoke",
        "When the work item is a task in the todo queue with the todo label, choose the advance action.",
    );
    let role_context = role_context(&role);
    let role_start = Instant::now();
    let decision: RoleDecision = runtime
        .block_on(run_decision(
            &provider,
            &role.prompt.render(),
            &role_context,
        ))
        .expect("ChatGPT OAuth generic role decision succeeds and parses");
    let role_latency = role_start.elapsed();
    eprintln!("[a3] generic role decision: {decision:?} (latency: {role_latency:?})");
    assert_eq!(decision.action, "advance");

    // Step 3 — refresh path: copy the real auth file, force its codex entry to
    // near-expiry, then run a decision through the copy. `resolve_bearer` must
    // refresh against the real token endpoint, the decision must still succeed,
    // and the rewritten copy must stay in its original (nodejs) on-disk schema.
    validate_refresh(&runtime);
}

/// Exercises the near-expiry refresh path on a throwaway copy of the real auth
/// file, then syncs the refreshed credential back to the real file so its
/// (possibly rotated) refresh token stays current for the next run. No token
/// bytes are ever printed.
fn validate_refresh(runtime: &tokio::runtime::Runtime) {
    let real_path = default_auth_path();
    let original = std::fs::read_to_string(&real_path).expect("real auth file is readable");
    let original_schema = codex_schema(&original);
    assert_eq!(
        original_schema, "nodejs",
        "this machine's auth.json is written by the nodejs pi; the refresh test asserts the \
         schema is preserved across a write-back"
    );

    let copy = TempAuthFile::seed(&original);
    // Force the codex entry to expire ~1s ago so `is_expiring` fires.
    let near_past = now_ms() - 1_000;
    copy.set_codex_expires(near_past);
    assert!(
        copy.codex_expires() <= now_ms(),
        "the copy's codex token is forced to near-past expiry"
    );

    let provider = ProviderConfig::chatgpt_oauth(None, Some(copy.path.clone()));
    let pong: Pong = runtime
        .block_on(run_decision(
            &provider,
            "You reply with a single JSON object and nothing else.",
            r#"Reply with exactly {"reply":"pong"}."#,
        ))
        .expect("decision after a forced refresh succeeds and parses");
    assert_eq!(pong.reply.trim().to_lowercase(), "pong");

    // The refresh must have rewritten the copy: expiry pushed into the future and
    // the on-disk schema unchanged (nodejs stays nodejs).
    let rewritten = std::fs::read_to_string(&copy.path).expect("rewritten copy is readable");
    assert_eq!(
        codex_schema(&rewritten),
        "nodejs",
        "a nodejs-schema file must stay nodejs-shaped after a refresh write-back"
    );
    let new_expires = copy.codex_expires();
    assert!(
        new_expires > now_ms(),
        "refresh must push the codex expiry into the future"
    );
    eprintln!(
        "[a3] refresh succeeded; expiry advanced by ~{} min, schema preserved (nodejs)",
        (new_expires - near_past) / 60_000
    );

    // OpenAI may rotate the refresh token on use, which would invalidate the
    // original now stored in the real file. Sync the refreshed copy back so the
    // real auth file stays usable for the next run. The write preserves schema.
    std::fs::copy(&copy.path, &real_path).expect("syncing the refreshed credential back succeeds");
    eprintln!("[a3] synced refreshed credential back to the real auth file (schema preserved)");
}

/// A self-cleaning copy of the auth file under the temp dir.
struct TempAuthFile {
    path: PathBuf,
}

impl TempAuthFile {
    fn seed(contents: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "smith-temper-agent-a3-auth-{}.json",
            std::process::id()
        ));
        std::fs::write(&path, contents).expect("seed temp auth file");
        Self { path }
    }

    fn set_codex_expires(&self, expires_ms: i64) {
        let mut root: Value =
            serde_json::from_str(&std::fs::read_to_string(&self.path).unwrap()).unwrap();
        root["openai-codex"]["expires"] = Value::from(expires_ms);
        std::fs::write(&self.path, serde_json::to_string_pretty(&root).unwrap()).unwrap();
    }

    fn codex_expires(&self) -> i64 {
        let root: Value =
            serde_json::from_str(&std::fs::read_to_string(&self.path).unwrap()).unwrap();
        root["openai-codex"]["expires"].as_i64().unwrap()
    }
}

impl Drop for TempAuthFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Classifies the `openai-codex` entry's on-disk schema by field spelling,
/// without ever touching the token bytes.
fn codex_schema(contents: &str) -> &'static str {
    let root: Value = serde_json::from_str(contents).expect("auth file parses as JSON");
    let entry = &root["openai-codex"];
    if entry.get("access").is_some() {
        "nodejs"
    } else if entry.get("access_token").is_some() {
        "rust"
    } else {
        "unknown"
    }
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}
