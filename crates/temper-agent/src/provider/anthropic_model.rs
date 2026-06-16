//! Anthropic model-id selection, request-identity headers, and per-model limit
//! tables for the OAuth path.
//!
//! These are the model-/tier-dependent data tables the Anthropic OAuth provider
//! consults: which model id to target (main vs. sub-agent), which
//! `anthropic-beta` flags a model is entitled to, and the output-token /
//! context-window ceilings the API enforces per model family. The bearer
//! resolution and auth-file handling live in the sibling `anthropic_oauth`
//! module, which re-exports this module's public surface.

use std::collections::HashMap;

use uuid::Uuid;

/// Env var overriding the Anthropic model id. The name lives here so the agent's
/// `entry` (the sole env reader) and the worker's env-injection map agree;
/// nothing in this crate reads it.
pub const ANTHROPIC_MODEL_ENV: &str = "TEMPER_AGENTS_ANTHROPIC_MODEL";
/// Env var overriding the Anthropic model id used for *sub-agents* (the
/// read-only `investigate` fan-out). Defaults to
/// [`DEFAULT_ANTHROPIC_SUBAGENT_MODEL`]. Read only by the agent's `entry`.
pub const ANTHROPIC_SUBAGENT_MODEL_ENV: &str = "TEMPER_AGENTS_ANTHROPIC_SUBAGENT_MODEL";

/// Default Anthropic model targeted by the OAuth mode (overridable).
///
/// `claude-opus-4-8` is the model the standard subscription tier actually
/// serves over this OAuth path: requesting `claude-fable-5` on a non-Mythos
/// subscription returns `404 "Claude Fable 5 is not available. Please use
/// Opus 4.8."`. The Claude Code CLI hides this by transparently falling back;
/// anvil sends the literal id, so the default must be a model the tier grants.
/// Override with `TEMPER_AGENTS_ANTHROPIC_MODEL` when a tier with Fable access
/// is in use.
pub const DEFAULT_ANTHROPIC_MODEL: &str = "claude-opus-4-8";
/// Default Anthropic model for read-only `investigate` sub-agents.
///
/// Sub-agents do mechanical search-and-read work whose product is a focused
/// report, not the final deliverable, so they run on a cheaper, faster model
/// than the main agent — mirroring Claude Code, which routes its `Explore`
/// investigation sub-agents to Haiku while keeping the orchestrator on Opus.
/// On a large fan-out the sub-agents dominate token spend, so this is the main
/// efficiency lever. `claude-haiku-4-5` is served on the standard subscription
/// tier (verified live). Override with [`ANTHROPIC_SUBAGENT_MODEL_ENV`]; set it
/// equal to the main model to disable tiering.
pub const DEFAULT_ANTHROPIC_SUBAGENT_MODEL: &str = "claude-haiku-4-5";
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

/// Beta flags sent for every Anthropic OAuth model.
const ANTHROPIC_BETA_BASE: &str = concat!(
    "claude-code-20250219,",
    "oauth-2025-04-20,",
    "interleaved-thinking-2025-05-14,",
    "context-management-2025-06-27,",
    "prompt-caching-scope-2026-01-05,",
    "advisor-tool-2026-03-01,",
    "advanced-tool-use-2025-11-20,",
    "effort-2025-11-24,",
    "extended-cache-ttl-2025-04-11"
);

/// The 1M-context beta, appended only for models/tiers that grant it. Requesting
/// it for a model the subscription does not entitle (e.g. Haiku on the standard
/// tier) is rejected with `400 "The long context beta is not yet available for
/// this subscription."`, which would fail every request on that model.
const ANTHROPIC_BETA_LONG_CONTEXT: &str = "context-1m-2025-08-07";

/// Whether `model_id` may request the 1M-context beta. Conservative: only the
/// larger models (Opus/Sonnet) that this subscription serves with long context.
/// The Haiku family is excluded (it 400s), as is any unknown id.
fn supports_long_context_beta(model_id: &str) -> bool {
    let id = model_id.to_ascii_lowercase();
    id.contains("opus") || id.contains("sonnet")
}

/// The `anthropic-beta` header value for `model_id`.
fn anthropic_beta_for(model_id: &str) -> String {
    if supports_long_context_beta(model_id) {
        format!("{ANTHROPIC_BETA_BASE},{ANTHROPIC_BETA_LONG_CONTEXT}")
    } else {
        ANTHROPIC_BETA_BASE.to_string()
    }
}

/// Resolves the Anthropic model id from the supplied override or the default.
///
/// `override_id` is the [`ANTHROPIC_MODEL_ENV`] value the host read (`None` when
/// unset); an empty/whitespace value is treated as unset. Reads no environment.
pub fn resolve_anthropic_model(override_id: Option<String>) -> String {
    override_id
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_ANTHROPIC_MODEL.to_string())
}

/// The maximum `max_tokens` (output) the Anthropic API accepts for `model_id`.
///
/// The API returns a 400 `invalid_request_error` when `max_tokens` exceeds the
/// model's ceiling, so this must track each model family. Opus/Sonnet 4.x serve
/// up to 128K output; the Haiku 4.x family caps at 64K. Unknown ids fall back to
/// the conservative 64K so a new/cheaper model never over-asks.
pub fn max_output_tokens_for(model_id: &str) -> usize {
    let id = model_id.to_ascii_lowercase();
    if id.contains("haiku") {
        64_000
    } else if id.contains("opus") || id.contains("sonnet") {
        128_000
    } else {
        64_000
    }
}

/// The context-window size to declare for `model_id`. Models granted the
/// 1M-context beta advertise 1M; the rest use the standard 200K window. This is
/// a local SDK hint (used for budgeting), kept consistent with the beta flags.
pub fn context_window_for(model_id: &str) -> usize {
    if supports_long_context_beta(model_id) {
        1_000_000
    } else {
        200_000
    }
}

/// Resolves the Anthropic sub-agent model id from the supplied override or the
/// default.
///
/// `override_id` is the [`ANTHROPIC_SUBAGENT_MODEL_ENV`] value the host read
/// (`None` when unset); an empty/whitespace value is treated as unset. Reads no
/// environment.
pub fn resolve_anthropic_subagent_model(override_id: Option<String>) -> String {
    override_id
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_ANTHROPIC_SUBAGENT_MODEL.to_string())
}

/// Claude Code-compatible headers injected per Anthropic OAuth request.
///
/// `model_id` selects the `anthropic-beta` flag set: the 1M-context beta is sent
/// only for models the subscription entitles (see [`anthropic_beta_for`]).
pub fn request_headers(model_id: &str) -> HashMap<String, String> {
    HashMap::from([
        (
            "x-client-request-id".to_string(),
            Uuid::new_v4().to_string(),
        ),
        ("anthropic-beta".to_string(), anthropic_beta_for(model_id)),
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
