// SPDX-License-Identifier: MPL-2.0

//! `temper run` — the solo-dev all-in-one: daemon + worker + agent in one
//! process on one event loop.
//!
//! Reuses the daemon's config parser for the orchestration side and adds a small
//! set of agent/worker flags. The forge connection comes from the environment
//! (FORGEJO_URL + FORGEJO_ACCESS_TOKEN), like the split daemon.

use std::path::PathBuf;
use std::process::ExitCode;

use temper_agent_runtime::{AuthChoice, DEFAULT_MAX_ITERATIONS, ProviderConfig};

use crate::run::{AllInOneConfig, run_all_in_one};

/// Hermetic-test hook: when this env var is set, the agent's provider traffic is
/// redirected to a local fake LLM (e.g. jig's `FakeLlm`) instead of the real
/// endpoint. Compiled only under `test-provider-base-url-override`.
#[cfg(feature = "test-provider-base-url-override")]
const TEST_PROVIDER_BASE_URL_ENV: &str = "ANVIL_TEST_PROVIDER_BASE_URL";

#[cfg(feature = "test-provider-base-url-override")]
fn apply_test_provider_base_url_override(provider: ProviderConfig) -> ProviderConfig {
    match std::env::var(TEST_PROVIDER_BASE_URL_ENV) {
        Ok(base_url) if !base_url.trim().is_empty() => provider.with_base_url_override(base_url),
        _ => provider,
    }
}

const USAGE: &str = concat!(
    "temper run --repo <owner/name> [--repo ...] --role <role> [--role ...]\n",
    "  [--workflow <path>] [--poll-cadence-secs <n>] [--lease-ttl-secs <n>]\n",
    "  [--daemon-id <id>] [--worker-id <id>]\n",
    "  [--auth <deepseek|chatgpt-oauth|anthropic-oauth>] [--auth-file <path>]\n",
    "  [--codex-model <id>] [--max-iterations <n>] [--config-dir <path>]\n",
    "  [--enable-subagents] [--workspace-root <path>] [--git-base-url <url>]\n",
    "\n",
    "  Runs the daemon, one worker, and the coding agent in a single process.\n",
    "  Forge connection comes from FORGEJO_URL + FORGEJO_ACCESS_TOKEN. Role git\n",
    "  identities come from TEMPER_FORGEJO_{USER,EMAIL,TOKEN}_<ROLE>."
);

pub fn main<I>(args: I) -> ExitCode
where
    I: Iterator<Item = String>,
{
    match run(args.collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("temper run: {error}");
            ExitCode::from(2)
        }
    }
}

struct RunExtras {
    auth: AuthChoice,
    auth_file: Option<PathBuf>,
    codex_model: Option<String>,
    max_iterations: usize,
    config_dir: Option<PathBuf>,
    enable_subagents: bool,
    workspace_root: PathBuf,
    git_base_url: Option<String>,
    worker_id: String,
}

impl Default for RunExtras {
    fn default() -> Self {
        Self {
            // Dev default mirrors the agent binary's default.
            auth: AuthChoice::ChatGptOAuth,
            auth_file: None,
            codex_model: None,
            max_iterations: DEFAULT_MAX_ITERATIONS,
            config_dir: None,
            enable_subagents: false,
            workspace_root: PathBuf::from(".temper/workspaces"),
            git_base_url: None,
            worker_id: "temper-run-1".to_string(),
        }
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return Ok(());
    }

    // Split the run-only flags out, leaving the rest for the daemon parser.
    let (daemon_args, extras) = split_args(args)?;

    let daemon_config = match temper_daemon::config::parse(daemon_args) {
        Ok(temper_daemon::config::ParseOutcome::Run(config)) => config,
        Ok(temper_daemon::config::ParseOutcome::Help) => {
            println!("{USAGE}");
            return Ok(());
        }
        Err(error) => return Err(format!("{error}\nusage: {USAGE}")),
    };

    let provider = ProviderConfig::from_auth(extras.auth, extras.codex_model, extras.auth_file)
        .map_err(|error| format!("provider preflight: {error}"))?;
    #[cfg(feature = "test-provider-base-url-override")]
    let provider = apply_test_provider_base_url_override(provider);

    // Default the agent's git base URL to the daemon's forge URL when not given.
    let git_base_url = match extras.git_base_url {
        Some(url) => url,
        None => std::env::var("FORGEJO_URL")
            .map_err(|_| "missing --git-base-url and FORGEJO_URL is unset".to_string())?,
    };

    let config = AllInOneConfig {
        daemon: daemon_config,
        provider,
        workspace_root: extras.workspace_root,
        git_base_url,
        max_iterations: extras.max_iterations,
        config_dir: extras.config_dir,
        enable_subagents: extras.enable_subagents,
        worker_id: extras.worker_id,
    };

    temper_io_engine::block_on_with(move |_cx, handle| async move {
        run_all_in_one(handle, config).await
    })
}

/// Peel the `run`-only flags off the argument list, passing the rest through to
/// the daemon parser.
fn split_args(args: Vec<String>) -> Result<(Vec<String>, RunExtras), String> {
    let mut extras = RunExtras::default();
    let mut daemon_args = Vec::new();
    let mut iter = args.into_iter();
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--auth" => {
                extras.auth = parse_auth(&value(&flag, &mut iter)?)?;
            }
            "--auth-file" => extras.auth_file = Some(PathBuf::from(value(&flag, &mut iter)?)),
            "--codex-model" => extras.codex_model = Some(value(&flag, &mut iter)?),
            "--max-iterations" => {
                extras.max_iterations = value(&flag, &mut iter)?
                    .parse()
                    .map_err(|_| "--max-iterations must be a positive integer".to_string())?;
            }
            "--config-dir" => extras.config_dir = Some(PathBuf::from(value(&flag, &mut iter)?)),
            "--enable-subagents" => extras.enable_subagents = true,
            "--workspace-root" => extras.workspace_root = PathBuf::from(value(&flag, &mut iter)?),
            "--git-base-url" => extras.git_base_url = Some(value(&flag, &mut iter)?),
            "--worker-id" => extras.worker_id = value(&flag, &mut iter)?,
            // Everything else (including its value, if any) goes to the daemon
            // parser. Flags the daemon parser owns are value-taking, so we pass
            // the flag and let the daemon parser consume its own value on its
            // next iteration — but since we are iterating here, forward both.
            _ => daemon_args.push(flag),
        }
    }
    Ok((daemon_args, extras))
}

fn value<I: Iterator<Item = String>>(flag: &str, iter: &mut I) -> Result<String, String> {
    iter.next()
        .ok_or_else(|| format!("flag '{flag}' expects a value"))
}

fn parse_auth(value: &str) -> Result<AuthChoice, String> {
    match value {
        "deepseek" => Ok(AuthChoice::DeepSeek),
        "chatgpt-oauth" => Ok(AuthChoice::ChatGptOAuth),
        "anthropic-oauth" => Ok(AuthChoice::AnthropicOAuth),
        other => Err(format!(
            "unknown --auth `{other}` (expected deepseek|chatgpt-oauth|anthropic-oauth)"
        )),
    }
}
