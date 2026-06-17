// SPDX-License-Identifier: MPL-2.0

//! The agent binary's entry point — the **single** env-reading module.
//!
//! The worker spawns the agent as a per-job process and injects its config
//! through the environment (a legitimate worker→agent IPC boundary). This module
//! is the one place that env is read: it parses the injected variables and the
//! CLI flags into an [`AgentConfig`] (plus the context/result paths and cwd),
//! then hands those resolved values to [`crate::run::drive`]. Every deeper
//! module in this crate and in `temper-agent` takes values from the config and
//! reads no environment — enforced by the `entry_is_sole_env_reader` test below.
//!
//! The env-var *names* live next to the code that consumes their values (the
//! provider constants in `temper-agent`, the protocol path constants in
//! `temper-protocol-agent`, and the checkpoint constants below), so this module
//! reads them by their canonical names rather than re-spelling the strings.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use secrecy::SecretString;
use temper_agent::{
    ANTHROPIC_MODEL_ENV, ANTHROPIC_SUBAGENT_MODEL_ENV, ANTHROPIC_TOKEN_URL_ENV, API_KEY_ENV,
    API_KEY_PATH_ENV, AUTH_FILE_ENV, CODEX_MODEL_ENV, CODEX_TOKEN_URL_ENV, CONFIG_DIR_ENV,
    PROVIDER_BASE_URL_ENV, ProviderConfig, ProviderEnv, XDG_CONFIG_HOME_ENV,
    resolve_config_dir_from,
};
use temper_protocol_agent::{CONTEXT_ENV, RESULT_ENV, WorkspaceContext};

use crate::config::{AgentConfig, RoleIdentity};
use crate::options::Options;

/// Env-var prefix for the per-role git author name.
const FORGEJO_USER_PREFIX: &str = "TEMPER_FORGEJO_USER";
/// Env-var prefix for the per-role git author email.
const FORGEJO_EMAIL_PREFIX: &str = "TEMPER_FORGEJO_EMAIL";
/// Env-var prefix for the per-role git push token.
const FORGEJO_TOKEN_PREFIX: &str = "TEMPER_FORGEJO_TOKEN";
/// Env var carrying the job deadline (lease expiry) as a unix-seconds timestamp.
const DEADLINE_ENV: &str = "TEMPER_AGENT_DEADLINE";
/// Env var carrying the checkpoint backstop cadence, in seconds.
const CHECKPOINT_INTERVAL_ENV: &str = "TEMPER_AGENT_CHECKPOINT_INTERVAL_SECS";
/// The `HOME` env var, the lowest-precedence config-dir source.
const HOME_ENV: &str = "HOME";

/// Reads the injected env + CLI flags and drives one protocol run.
pub(crate) fn run<I>(args: I) -> Result<(), String>
where
    I: Iterator<Item = String>,
{
    let options = Options::parse(args)?;

    let result_path = var(RESULT_ENV)
        .ok_or_else(|| format!("missing required env var {RESULT_ENV} (result file path)"))?;
    let context = read_context()?;
    let config = build_config(options, &context)?;

    // The checkout is our cwd: the worker runs us there, exactly as the legacy
    // file-protocol coder was run.
    let cwd = std::env::current_dir().map_err(|error| format!("resolve cwd: {error}"))?;

    crate::run::drive(config, context, cwd, result_path)
}

/// Reads and parses the [`WorkspaceContext`] from the file named by
/// [`CONTEXT_ENV`].
fn read_context() -> Result<WorkspaceContext, String> {
    let context_path = var(CONTEXT_ENV)
        .ok_or_else(|| format!("missing required env var {CONTEXT_ENV} (context file path)"))?;
    let context_bytes = std::fs::read(&context_path)
        .map_err(|error| format!("read context file {context_path}: {error}"))?;
    serde_json::from_slice(&context_bytes)
        .map_err(|error| format!("parse context file {context_path}: {error}"))
}

/// Assembles the agent's per-subsystem [`AgentConfig`] from the parsed CLI
/// options plus the injected environment.
///
/// Provider/model/credential wiring comes from the CLI flags (`--auth`, …) and
/// the worker-injected env (model ids, base-URL override, auth-file path, token
/// endpoints, the DeepSeek key) gathered into a [`ProviderEnv`]. The base-URL
/// override (`ANVIL_TEST_PROVIDER_BASE_URL`) is applied last so it wins over any
/// config URL. The deadline/checkpoint-cadence and per-role git identity are read
/// here and layered onto the config; nothing deeper reads env.
fn build_config(options: Options, context: &WorkspaceContext) -> Result<AgentConfig, String> {
    let provider_env = read_provider_env();
    let provider = ProviderConfig::from_auth(
        options.auth,
        options.codex_model,
        options.auth_file,
        &provider_env,
    )
    .map_err(|error| format!("provider preflight: {error}"))?
    .apply_base_url_override(provider_env.base_url_override.as_deref());

    let config_dir = resolve_config_dir_from(
        options.config_dir.as_deref(),
        path_var(CONFIG_DIR_ENV),
        path_var(XDG_CONFIG_HOME_ENV),
        path_var(HOME_ENV),
    );

    Ok(AgentConfig::new(
        provider,
        options.max_iterations,
        options.enable_subagents,
        config_dir,
    )
    .with_deadline(read_deadline())
    .with_checkpoint_interval(read_checkpoint_interval())
    .with_role_identity(read_role_identity(&context.work_item.role)))
}

/// Gathers the provider/credential inputs from the injected environment.
fn read_provider_env() -> ProviderEnv {
    ProviderEnv {
        deepseek_api_key: var(API_KEY_ENV).map(SecretString::from),
        deepseek_api_key_path: path_var(API_KEY_PATH_ENV),
        auth_file: path_var(AUTH_FILE_ENV),
        codex_model: var(CODEX_MODEL_ENV),
        codex_token_url: var(CODEX_TOKEN_URL_ENV),
        anthropic_model: var(ANTHROPIC_MODEL_ENV),
        anthropic_subagent_model: var(ANTHROPIC_SUBAGENT_MODEL_ENV),
        anthropic_token_url: var(ANTHROPIC_TOKEN_URL_ENV),
        base_url_override: var(PROVIDER_BASE_URL_ENV),
    }
}

/// The job deadline (lease expiry) from [`DEADLINE_ENV`], if the host (which owns
/// the lease) passed one as a unix-seconds timestamp.
fn read_deadline() -> Option<SystemTime> {
    var(DEADLINE_ENV)
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(|secs| UNIX_EPOCH + Duration::from_secs(secs))
}

/// The checkpoint backstop cadence from [`CHECKPOINT_INTERVAL_ENV`], falling back
/// to the session default when unset.
fn read_checkpoint_interval() -> Duration {
    var(CHECKPOINT_INTERVAL_ENV)
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(crate::DEFAULT_CHECKPOINT_INTERVAL)
}

/// The per-role git identity + push token from the
/// `TEMPER_FORGEJO_{USER,EMAIL,TOKEN}_<ROLE>` env (the `<ROLE>` suffix derives
/// from the work item's role). Absent values degrade gracefully downstream.
fn read_role_identity(role: &str) -> RoleIdentity {
    let role_key = role.to_uppercase().replace('-', "_");
    RoleIdentity {
        user: var(&format!("{FORGEJO_USER_PREFIX}_{role_key}")),
        email: var(&format!("{FORGEJO_EMAIL_PREFIX}_{role_key}")),
        token: var(&format!("{FORGEJO_TOKEN_PREFIX}_{role_key}")).map(SecretString::from),
    }
}

/// Reads an env var, trimming and treating empty/whitespace as unset.
fn var(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Reads an env var as a [`PathBuf`], treating empty as unset (matching the XDG
/// convention that an empty value means "use the default").
fn path_var(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    /// `entry` (plus the binary shim) is the only module in this crate and in
    /// `temper-agent` that reads `std::env`. This greps the two crates' sources to
    /// keep the env-confinement invariant honest as code is added (the #202 CI
    /// lint will enforce the same rule workspace-wide).
    #[test]
    fn entry_is_sole_env_reader() {
        let crates_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates dir");
        let needles = ["std::env", "env::var", "env::var_os", "env::vars"];
        let mut offenders = Vec::new();
        for crate_name in ["temper-agent", "temper-agent-session"] {
            let src = crates_dir.join(crate_name).join("src");
            scan_dir(&src, &needles, &mut offenders);
        }
        assert!(
            offenders.is_empty(),
            "env reads found outside entry.rs / the binary shim:\n{}",
            offenders.join("\n")
        );
    }

    /// Recursively scans `dir`'s `.rs` files for env-read needles, allowing only
    /// the entry module and the binary shim. Test sources are excluded wholesale:
    /// `tests/` subdirectories (integration/jig helpers) and `*_tests.rs` unit
    /// modules legitimately use `std::env::temp_dir` for scratch paths.
    fn scan_dir(dir: &Path, needles: &[&str], offenders: &mut Vec<String>) {
        // Skip any `tests/` subtree (its `.rs` files are test support).
        if dir.file_name().and_then(|n| n.to_str()) == Some("tests") {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                scan_dir(&path, needles, offenders);
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            // Allowed env readers: this entry module and the binary shim.
            if name == "entry.rs" || name == "temper-agent.rs" || name.ends_with("_tests.rs") {
                continue;
            }
            let Ok(body) = std::fs::read_to_string(&path) else {
                continue;
            };
            for (lineno, line) in body.lines().enumerate() {
                let trimmed = line.trim_start();
                // Skip line and doc comments — the env-var *names* are documented
                // next to the code that consumes them.
                if trimmed.starts_with("//") {
                    continue;
                }
                if needles.iter().any(|needle| line.contains(needle)) {
                    offenders.push(format!(
                        "{}:{}: {}",
                        path.display(),
                        lineno + 1,
                        line.trim()
                    ));
                }
            }
        }
    }
}
