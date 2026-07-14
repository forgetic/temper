// SPDX-License-Identifier: MPL-2.0

//! The agent binary's entry point — the **single** env-reading module.
//!
//! Every non-secret input is a CLI flag (parsed by [`crate::options`]); the one
//! secret, the provider credential, is the only environment variable the agent
//! reads: [`PROVIDER_CREDENTIALS_ENV`]
//! (`TEMPER_AGENT_PROVIDER_CREDENTIALS_JSON`). This module parses that JSON and
//! the flags into an [`AgentConfig`] (plus the context/result/workspace paths),
//! then hands the resolved values to [`crate::run::drive`]. Every deeper module
//! in this crate and in `temper-agent` takes values from the config and reads no
//! environment — enforced by the `entry_is_sole_env_reader` test below.
//!
//! `HOME`/`XDG_CONFIG_HOME` may still be consulted, but only as a fallback for
//! the prompt-overlay/capture dir when `--capture-dir` is not given.
//!
//! For OAuth providers the credential carries the tokens inline; this module
//! materializes them into a pi-format `auth.json` in a temp dir that the agent's
//! OAuth loader reads (and refreshes) exactly as before, so the heavy OAuth
//! machinery is unchanged — only the *source* of the tokens moved from a
//! worker-written file to the credential JSON.

use std::path::PathBuf;

use temper_agent::{
    AuthChoice, ProviderConfig, ProviderEnv, XDG_CONFIG_HOME_ENV, resolve_config_dir_from,
};
use temper_protocol_activity::AgentActivityCapturePolicyV1;
use temper_protocol_agent::{
    AgentToolConfig, PROVIDER_CREDENTIALS_ENV, ProviderCredentialJson, WorkspaceContext,
};

use crate::config::AgentConfig;
use crate::options::Options;

/// The `HOME` env var, the lowest-precedence capture-dir source.
const HOME_ENV: &str = "HOME";

/// Reads the one secret env var + the CLI flags and drives one protocol run.
pub(crate) fn run<I>(args: I) -> Result<(), String>
where
    I: Iterator<Item = String>,
{
    let options = match Options::parse(args)? {
        Some(options) => options,
        None => {
            println!("{}", crate::options::USAGE);
            return Ok(());
        }
    };
    let context = read_context(&options)?;

    // The workspace (checkout) defaults to the process cwd: the worker runs us
    // there, exactly as the legacy file-protocol coder was run.
    let cwd = match &options.workspace {
        Some(workspace) => workspace.clone(),
        None => std::env::current_dir().map_err(|error| format!("resolve cwd: {error}"))?,
    };
    let result_path = options.result.display().to_string();

    // The OAuth auth.json (when an OAuth credential is in play) lives in this
    // temp dir for the whole run, so the OAuth loader can read/refresh it across
    // model calls; it is removed when `_auth_dir` drops at the end of the run.
    let (config, _auth_dir) = build_config(options, &context)?;

    crate::run::drive(config, context, cwd, result_path)
}

/// Reads and parses the [`WorkspaceContext`] from the `--context` file.
fn read_context(options: &Options) -> Result<WorkspaceContext, String> {
    let context_path = &options.context;
    let context_bytes = std::fs::read(context_path)
        .map_err(|error| format!("read context file {}: {error}", context_path.display()))?;
    serde_json::from_slice(&context_bytes)
        .map_err(|error| format!("parse context file {}: {error}", context_path.display()))
}

/// Assembles the agent's per-subsystem [`AgentConfig`] from the parsed CLI
/// options plus the one secret env var (the provider credential).
///
/// Provider/model/url wiring comes from the flags; the credential comes from
/// [`PROVIDER_CREDENTIALS_ENV`]. For OAuth credentials the tokens are
/// materialized into a pi-format `auth.json` whose [`tempfile::TempDir`] is
/// returned so it outlives the run. Nothing deeper reads env.
fn build_config(
    options: Options,
    _context: &WorkspaceContext,
) -> Result<(AgentConfig, Option<tempfile::TempDir>), String> {
    let credential = read_credential()?;
    let (provider, auth_dir) = build_provider(&options, credential)?;
    if options.activity_address.is_some() {
        tracing::debug!(
            target: "temper::agent",
            "worker activity endpoint supplied for this agent run"
        );
    }

    let config_dir = resolve_config_dir_from(
        options.capture_dir.as_deref(),
        None,
        path_var(XDG_CONFIG_HOME_ENV),
        path_var(HOME_ENV),
    );

    let config = AgentConfig::new(
        provider,
        options.max_iterations,
        options.subagents,
        config_dir,
    )
    .with_tool_config(read_tool_config(options.tool_config.as_deref())?)
    .with_trace_policy(read_trace_policy(options.trace_policy.as_deref())?);
    let config = match options.submit_for_pr_address {
        Some(address) => config.with_submit_for_pr(crate::submit_client::host_for_address(address)),
        None => config,
    };
    let config = match options.forge_context_address {
        Some(address) => config.with_forge_context(crate::forge_client::host_for_address(address)),
        None => config,
    };
    Ok((config, auth_dir))
}

/// Reads and validates the optional non-secret agent tool config JSON file.
fn read_tool_config(path: Option<&std::path::Path>) -> Result<Option<AgentToolConfig>, String> {
    let Some(path) = path else {
        return Ok(None);
    };
    let raw = std::fs::read_to_string(path)
        .map_err(|error| format!("read tool config file {}: {error}", path.display()))?;
    AgentToolConfig::from_json(&raw)
        .map(Some)
        .map_err(|error| format!("parse tool config file {}: {error}", path.display()))
}

/// Reads and validates the non-secret shared capture-policy DTO. An absent file
/// preserves the protocol's metadata-capture default for direct agent runs.
fn read_trace_policy(
    path: Option<&std::path::Path>,
) -> Result<AgentActivityCapturePolicyV1, String> {
    let Some(path) = path else {
        return Ok(AgentActivityCapturePolicyV1::default());
    };
    let raw = std::fs::read_to_string(path)
        .map_err(|error| format!("read trace policy file {}: {error}", path.display()))?;
    let policy: AgentActivityCapturePolicyV1 = serde_json::from_str(&raw)
        .map_err(|error| format!("parse trace policy file {}: {error}", path.display()))?;
    policy
        .validate()
        .map_err(|error| format!("validate trace policy file {}: {error}", path.display()))?;
    Ok(policy)
}

/// Builds the [`ProviderConfig`] from the flags + the parsed credential.
///
/// `api-key` credentials build the DeepSeek (OpenAI-compatible) config directly
/// with the key. `oauth` credentials are materialized into a temp `auth.json`
/// the existing OAuth loader reads; the temp dir is returned so it outlives the
/// run. The `--provider-url` override is applied last so it wins over any config
/// URL (pointing the agent at a local fake LLM in hermetic tests).
fn build_provider(
    options: &Options,
    credential: ProviderCredentialJson,
) -> Result<(ProviderConfig, Option<tempfile::TempDir>), String> {
    let env = provider_env_from_flags(options);
    let (config, auth_dir) = match credential {
        ProviderCredentialJson::ApiKey { api_key } => {
            // An api-key credential always drives the OpenAI-compatible DeepSeek
            // path, regardless of the `--provider` flag (only that path takes a
            // static key). No preflight is needed — the key is already in hand.
            let config = ProviderConfig::deepseek_with_key(
                api_key,
                options.model.clone(),
                options.provider_url.clone(),
            );
            (config, None)
        }
        ProviderCredentialJson::Oauth {
            access_token,
            refresh_token,
            expires_at_unix_seconds,
        } => {
            let auth_dir = tempfile::tempdir()
                .map_err(|error| format!("create agent auth temp dir: {error}"))?;
            let auth_file = materialize_oauth_auth_json(
                options.provider,
                &access_token,
                refresh_token.as_deref(),
                expires_at_unix_seconds,
                auth_dir.path(),
            )?;
            let config = ProviderConfig::from_auth(
                options.provider,
                options.model.clone(),
                Some(auth_file),
                &env,
            )
            .map_err(provider_error)?;
            (config, Some(auth_dir))
        }
    };
    Ok((
        config.apply_base_url_override(options.provider_url.as_deref()),
        auth_dir,
    ))
}

/// Gathers the value-carrying [`ProviderEnv`] from the CLI flags (it reads no
/// environment — the field names just mirror the historical env channel).
fn provider_env_from_flags(options: &Options) -> ProviderEnv {
    ProviderEnv {
        anthropic_model: options.model.clone(),
        anthropic_subagent_model: options.investigate_model.clone(),
        codex_model: options.model.clone(),
        base_url_override: options.provider_url.clone(),
        ..ProviderEnv::empty()
    }
}

/// Reads and parses the one secret env var, the provider credential.
fn read_credential() -> Result<ProviderCredentialJson, String> {
    let raw = var(PROVIDER_CREDENTIALS_ENV).ok_or_else(|| {
        format!("missing required env var {PROVIDER_CREDENTIALS_ENV} (provider credential JSON)")
    })?;
    // The parse error carries serde's message but never the secret bytes (serde
    // reports the offending key/type, not the credential value).
    ProviderCredentialJson::from_json(&raw)
        .map_err(|error| format!("parse {PROVIDER_CREDENTIALS_ENV}: {error}"))
}

/// Writes the OAuth tokens into a pi-format `auth.json` under `dir` and returns
/// its path, so the existing OAuth loader can read (and refresh) it.
///
/// The nodejs pi `auth.json` schema is `{ <key>: { type, access, refresh,
/// expires } }` with `expires` in unix **milliseconds** (the agent's tolerant
/// OAuth reader). The provider key derives from the chosen provider.
fn materialize_oauth_auth_json(
    provider: AuthChoice,
    access: &str,
    refresh: Option<&str>,
    expires_at_unix_seconds: i64,
    dir: &std::path::Path,
) -> Result<PathBuf, String> {
    let key = oauth_provider_key(provider);
    let entry = serde_json::json!({
        key: {
            "type": "oauth",
            "access": access,
            "refresh": refresh,
            "expires": expires_at_unix_seconds.saturating_mul(1_000),
        }
    });
    let json = serde_json::to_string_pretty(&entry)
        .map_err(|error| format!("serialize agent auth.json: {error}"))?;
    let path = dir.join("auth.json");
    std::fs::write(&path, json)
        .map_err(|error| format!("write agent auth.json {}: {error}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("chmod agent auth.json {}: {error}", path.display()))?;
    }
    Ok(path)
}

/// The `auth.json` provider key an OAuth credential is stored under for a chosen
/// provider.
fn oauth_provider_key(provider: AuthChoice) -> &'static str {
    match provider {
        AuthChoice::ChatGptOAuth => "openai-codex",
        // DeepSeek never uses OAuth; the key is unused for it.
        AuthChoice::AnthropicOAuth | AuthChoice::DeepSeek => "anthropic",
    }
}

/// Renders a provider error for stderr. The provider stack already redacts
/// secrets (errors carry only the provider/path), so this is a thin wrapper.
fn provider_error(error: temper_agent::ProviderError) -> String {
    format!("provider preflight: {error}")
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
    use super::*;
    use std::path::Path;

    #[test]
    fn tool_config_absent_is_none() {
        assert!(read_tool_config(None).expect("absent ok").is_none());
    }

    #[test]
    fn tool_config_file_is_parsed() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("tools.json");
        std::fs::write(
            &path,
            r#"{
                "codebase_memory": {
                    "mode": "auto",
                    "command": "codebase-memory-mcp",
                    "roles": ["engineer"],
                    "index": "background",
                    "startup_timeout_secs": 5,
                    "index_timeout_secs": 30
                }
            }"#,
        )
        .expect("write tool config");

        let config = read_tool_config(Some(&path))
            .expect("tool config parses")
            .expect("tool config present");
        assert!(config.enabled_for_role("engineer"));
        assert!(!config.enabled_for_role("architect"));
    }

    #[test]
    fn tool_config_read_and_parse_failures_are_errors() {
        let temp = tempfile::tempdir().expect("tempdir");
        let missing = temp.path().join("missing.json");
        let missing_error = read_tool_config(Some(&missing)).expect_err("missing file fails");
        assert!(missing_error.contains("read tool config file"));

        let directory_error = read_tool_config(Some(temp.path())).expect_err("directory fails");
        assert!(directory_error.contains("read tool config file"));

        let invalid = temp.path().join("invalid.json");
        std::fs::write(&invalid, "not json").expect("write invalid json");
        let parse_error = read_tool_config(Some(&invalid)).expect_err("invalid json fails");
        assert!(parse_error.contains("parse tool config file"));
    }

    #[test]
    fn trace_policy_defaults_and_validates_worker_file() {
        assert_eq!(
            read_trace_policy(None).expect("default policy"),
            AgentActivityCapturePolicyV1::default()
        );

        let temp = tempfile::tempdir().expect("tempdir");
        let valid = temp.path().join("trace-policy.json");
        let mut policy = AgentActivityCapturePolicyV1 {
            capture: temper_protocol_activity::CaptureModeV1::Diagnostic,
            capture_thinking: true,
            ..Default::default()
        };
        std::fs::write(
            &valid,
            serde_json::to_vec(&policy).expect("serialize policy"),
        )
        .expect("write policy");
        assert_eq!(
            read_trace_policy(Some(&valid)).expect("valid policy"),
            policy
        );

        policy.capture = temper_protocol_activity::CaptureModeV1::Metadata;
        let invalid = temp.path().join("invalid-trace-policy.json");
        std::fs::write(
            &invalid,
            serde_json::to_vec(&policy).expect("serialize policy"),
        )
        .expect("write policy");
        let error = read_trace_policy(Some(&invalid)).expect_err("invalid policy fails");
        assert!(error.contains("capture_thinking"), "{error}");
    }

    /// `entry` (plus the binary shim) is the only module in this crate and in
    /// `temper-agent` that reads `std::env`, and it reads only the one secret
    /// credential var (plus HOME/XDG for the capture-dir fallback). This greps
    /// the two crates' sources to keep the env-confinement invariant honest as
    /// code is added (the CI lint enforces the same rule workspace-wide).
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
