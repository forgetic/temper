use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Duration;

use crate::workspace::RoleGitIdentity;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilitySpec {
    pub repo: String,
    pub role: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerConfig {
    pub daemon_url: String,
    pub worker_id: String,
    pub capabilities: Vec<CapabilitySpec>,
    /// How many jobs this worker runs at once.
    ///
    /// The design point is **one job at a time** (the default, and what the
    /// examples and dogfood deploy set): a worker claims a ticket, works it to
    /// completion, then claims the next — Forgejo-runner-shaped. A worker spends
    /// most of a job blocked on LLM latency, so to run more jobs in parallel the
    /// intended path is **more worker processes** (dozens is fine on one host),
    /// not more concurrency inside one worker. Values >1 are still honored — the
    /// capacity bookkeeping in [`crate::worker_machine::WorkerMachine`] is
    /// invariant-checked and fuzzed for any value — but they are not the design
    /// point. If a single worker ever genuinely needs several *top-level* agents,
    /// the cleaner move is a per-job fan-out (tag completions with a job id and
    /// route to a child machine) rather than relying on this knob.
    pub max_concurrent_jobs: u32,
    pub poll_wait: Duration,
    pub heartbeat_interval: Duration,
    pub executor: ExecutorSelection,
}

/// Default backoff before re-polling after the daemon returned no work, the
/// long-poll timed out, a transport error occurred, or the worker was at
/// capacity. Small so a freed slot is picked up promptly, but non-zero so an
/// idle or erroring worker does not hot-loop the daemon. (The steady-state pace
/// when work is flowing is set by the long-poll `max_wait_ms`, not this.)
pub const DEFAULT_POLL_BACKOFF: Duration = Duration::from_millis(500);

/// Identity + cadence knobs the pure [`WorkerMachine`](crate::worker_machine::WorkerMachine)
/// needs: a projection of [`WorkerConfig`] without the daemon URL, executor
/// selection, or any transport.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerParams {
    pub worker_id: String,
    pub capabilities: Vec<CapabilitySpec>,
    pub max_concurrent_jobs: u32,
    pub poll_wait: Duration,
    pub heartbeat_interval: Duration,
    pub poll_backoff: Duration,
}

impl WorkerParams {
    /// Projects a [`WorkerConfig`] into the machine's parameters, applying the
    /// default poll backoff.
    pub fn from_config(config: &WorkerConfig) -> Self {
        Self {
            worker_id: config.worker_id.clone(),
            capabilities: config.capabilities.clone(),
            max_concurrent_jobs: config.max_concurrent_jobs,
            poll_wait: config.poll_wait,
            heartbeat_interval: config.heartbeat_interval,
            poll_backoff: DEFAULT_POLL_BACKOFF,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutorSelection {
    Stub,
    Coding(CodingSurface),
}

/// The coding-executor surface: the workspace/git wiring plus how one agent
/// turn is produced ([`AgentSurface`]).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodingSurface {
    pub workspace_root: PathBuf,
    pub git_base_url: String,
    pub agent: AgentSurface,
}

/// How the coding executor produces an agent turn.
///
/// Both surfaces resolve to a **command the worker spawns out-of-process** over
/// the `smith-agent-protocol`. `--agent-command` selects the program: the
/// literal `anvil-native` selects the native anvil agent surface, which spawns
/// `anvil-agent` (overridable via `--agent-program`); any other value is
/// spawned verbatim (the examples' deterministic `greeting` stand-in, or an
/// operator-provided coder). Trailing `--agent-arg` values are the agent's
/// flags: for the anvil-native surface they are parsed here and re-rendered
/// onto the `anvil-agent` command (`--agent-program` / `--auth` / `--auth-file`
/// / `--codex-model` / `--max-iterations` / `--config-dir` / `--enable-subagents`);
/// for an external command they are passed through verbatim.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentSurface {
    /// The native anvil coding agent, spawned out-of-process as `anvil-agent`.
    AnvilNative(AnvilNativeAgentSurface),
    /// An external program spawned per job (program first, then args).
    ExternalCommand(Vec<String>),
}

/// Parsed configuration for the anvil-native agent surface — the flags the
/// out-of-process `anvil-agent` binary parses for itself.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnvilNativeAgentSurface {
    /// The agent program the worker spawns. Defaults to [`ANVIL_AGENT_PROGRAM`]
    /// (`anvil-agent`, resolved on `PATH`); override with an absolute path via
    /// `--agent-program` when it is not on `PATH`.
    pub agent_program: String,
    pub auth: AgentAuthChoice,
    pub codex_model: Option<String>,
    pub auth_file: Option<PathBuf>,
    pub config_dir: Option<PathBuf>,
    pub max_iterations: Option<usize>,
    /// Enable the in-workspace `investigate` sub-agent tool (off by default).
    pub enable_subagents: bool,
}

impl AnvilNativeAgentSurface {
    /// Renders the spawn command `OutOfProcessRunner` runs: the agent program
    /// followed by the same flags `anvil-agent` parses (`--auth`, `--auth-file`,
    /// `--codex-model`, `--config-dir`, `--max-iterations`, `--enable-subagents`).
    pub fn into_command(self) -> Vec<String> {
        let mut command = vec![self.agent_program];
        command.push("--auth".to_string());
        command.push(
            match self.auth {
                AgentAuthChoice::DeepSeek => "deepseek",
                AgentAuthChoice::ChatGptOAuth => "chatgpt-oauth",
                AgentAuthChoice::AnthropicOAuth => "anthropic-oauth",
            }
            .to_string(),
        );
        if let Some(codex_model) = self.codex_model {
            command.push("--codex-model".to_string());
            command.push(codex_model);
        }
        if let Some(auth_file) = self.auth_file {
            command.push("--auth-file".to_string());
            command.push(auth_file.to_string_lossy().into_owned());
        }
        if let Some(config_dir) = self.config_dir {
            command.push("--config-dir".to_string());
            command.push(config_dir.to_string_lossy().into_owned());
        }
        if let Some(max_iterations) = self.max_iterations {
            command.push("--max-iterations".to_string());
            command.push(max_iterations.to_string());
        }
        if self.enable_subagents {
            command.push("--enable-subagents".to_string());
        }
        command
    }
}

/// Which credential the agent authenticates with. Mirrors the agent's
/// `AuthChoice` but is parsed in the worker (which links no agent code); the
/// worker renders it back to the `--auth` flag in [`AnvilNativeAgentSurface::into_command`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentAuthChoice {
    DeepSeek,
    ChatGptOAuth,
    AnthropicOAuth,
}

/// The program name that selects the anvil-native out-of-process agent surface.
const ANVIL_NATIVE_PROGRAM: &str = "anvil-native";
/// The agent binary the anvil-native surface spawns by default.
pub const ANVIL_AGENT_PROGRAM: &str = "anvil-agent";

pub const USAGE: &str = "smith-worker --daemon-url <url> --worker-id <id> --capability <owner/name>:<role> [--capability ...] [--max-concurrent <n>] [--poll-wait-ms <n>] [--heartbeat-interval-ms <n>] [--executor <stub|coding>] [--workspace-root <path>] [--git-base-url <url>] [--agent-command <anvil-native|program>] [--agent-arg <arg> ...]\n  --agent-command anvil-native spawns the out-of-process anvil-agent; its --agent-arg values (--agent-program, --auth, --auth-file, --codex-model, --config-dir, --max-iterations, --enable-subagents) become the agent's flags. Any other --agent-command is spawned verbatim over the same protocol.";

// `Run(WorkerConfig)` is far larger than `Help`, but `ParseOutcome` is produced
// exactly once at process start and immediately destructured — the size
// difference never matters, and boxing would only obscure the config flow.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseOutcome {
    Help,
    Run(WorkerConfig),
}

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<ParseOutcome, String> {
    let args: Vec<String> = args.into_iter().collect();
    if contains_help_request(&args) {
        return Ok(ParseOutcome::Help);
    }

    let mut daemon_url: Option<String> = None;
    let mut worker_id: Option<String> = None;
    let mut capabilities = Vec::new();
    let mut seen_capabilities = BTreeSet::new();
    let mut max_concurrent_jobs = 1;
    let mut poll_wait_ms = 30_000;
    let mut heartbeat_interval_ms = 10_000;
    let mut executor = ExecutorKind::Stub;
    let mut workspace_root: Option<PathBuf> = None;
    let mut git_base_url: Option<String> = None;
    let mut agent_program: Option<String> = None;
    let mut agent_args = Vec::new();

    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        match arg.as_str() {
            "--daemon-url" => {
                let value = flag_value(&args, &mut index, "--daemon-url")?;
                let value = required_trimmed_value("--daemon-url", value)?;
                daemon_url = Some(value);
            }
            "--worker-id" => {
                let value = flag_value(&args, &mut index, "--worker-id")?;
                let value = required_trimmed_value("--worker-id", value)?;
                worker_id = Some(value);
            }
            "--capability" => {
                let value = flag_value(&args, &mut index, "--capability")?;
                let capability = parse_capability(value)?;
                let key = (capability.repo.clone(), capability.role.clone());
                if seen_capabilities.insert(key) {
                    capabilities.push(capability);
                }
            }
            "--max-concurrent" => {
                let value = flag_value(&args, &mut index, "--max-concurrent")?;
                max_concurrent_jobs = parse_non_zero_u32("--max-concurrent", value)?;
            }
            "--poll-wait-ms" => {
                let value = flag_value(&args, &mut index, "--poll-wait-ms")?;
                poll_wait_ms = parse_non_zero_u64("--poll-wait-ms", value)?;
            }
            "--heartbeat-interval-ms" => {
                let value = flag_value(&args, &mut index, "--heartbeat-interval-ms")?;
                heartbeat_interval_ms = parse_non_zero_u64("--heartbeat-interval-ms", value)?;
            }
            "--executor" => {
                let value = flag_value(&args, &mut index, "--executor")?;
                executor = parse_executor(value)?;
            }
            "--workspace-root" => {
                let value = flag_value(&args, &mut index, "--workspace-root")?;
                workspace_root = Some(PathBuf::from(required_trimmed_value(
                    "--workspace-root",
                    value,
                )?));
            }
            "--git-base-url" => {
                let value = flag_value(&args, &mut index, "--git-base-url")?;
                git_base_url = Some(required_trimmed_value("--git-base-url", value)?);
            }
            "--agent-command" => {
                let value = flag_value(&args, &mut index, "--agent-command")?;
                agent_program = Some(required_trimmed_value("--agent-command", value)?);
            }
            "--agent-arg" => {
                let value = positional_flag_value(&args, &mut index, "--agent-arg")?;
                agent_args.push(value.to_string());
            }
            other if other.starts_with('-') => return Err(format!("unknown flag `{other}`")),
            other => return Err(format!("unexpected positional argument `{other}`")),
        }
        index += 1;
    }

    let daemon_url = daemon_url.ok_or_else(|| "--daemon-url is required".to_string())?;
    let worker_id = worker_id.ok_or_else(|| "--worker-id is required".to_string())?;
    if capabilities.is_empty() {
        return Err("--capability is required at least once".to_string());
    }
    let executor = executor_selection(
        executor,
        workspace_root,
        git_base_url,
        agent_program,
        agent_args,
    )?;

    Ok(ParseOutcome::Run(WorkerConfig {
        daemon_url,
        worker_id,
        capabilities,
        max_concurrent_jobs,
        poll_wait: Duration::from_millis(poll_wait_ms),
        heartbeat_interval: Duration::from_millis(heartbeat_interval_ms),
        executor,
    }))
}

fn contains_help_request(args: &[String]) -> bool {
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--help" | "-h" => return true,
            "--agent-arg" => index += 1,
            _ => {}
        }
        index += 1;
    }
    false
}

fn flag_value<'a>(args: &'a [String], index: &mut usize, flag: &str) -> Result<&'a str, String> {
    *index += 1;
    let value = args
        .get(*index)
        .ok_or_else(|| format!("{flag} requires a value"))?;
    if value.starts_with('-') {
        return Err(format!("{flag} requires a value"));
    }
    Ok(value)
}

fn positional_flag_value<'a>(
    args: &'a [String],
    index: &mut usize,
    flag: &str,
) -> Result<&'a str, String> {
    *index += 1;
    args.get(*index)
        .map(String::as_str)
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn required_trimmed_value(flag: &str, value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("{flag} must not be empty"));
    }
    Ok(value.to_string())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExecutorKind {
    Stub,
    Coding,
}

fn parse_executor(value: &str) -> Result<ExecutorKind, String> {
    match value.trim() {
        "stub" => Ok(ExecutorKind::Stub),
        "coding" => Ok(ExecutorKind::Coding),
        other => Err(format!(
            "--executor must be `stub` or `coding` (got `{other}`)"
        )),
    }
}

fn executor_selection(
    executor: ExecutorKind,
    workspace_root: Option<PathBuf>,
    git_base_url: Option<String>,
    agent_program: Option<String>,
    agent_args: Vec<String>,
) -> Result<ExecutorSelection, String> {
    match executor {
        ExecutorKind::Stub => {
            if workspace_root.is_some() {
                return Err("--workspace-root requires --executor coding".to_string());
            }
            if git_base_url.is_some() {
                return Err("--git-base-url requires --executor coding".to_string());
            }
            if agent_program.is_some() {
                return Err("--agent-command requires --executor coding".to_string());
            }
            if !agent_args.is_empty() {
                return Err("--agent-arg requires --executor coding".to_string());
            }
            Ok(ExecutorSelection::Stub)
        }
        ExecutorKind::Coding => {
            let workspace_root = workspace_root
                .ok_or_else(|| "--workspace-root is required when --executor coding".to_string())?;
            let git_base_url = git_base_url
                .ok_or_else(|| "--git-base-url is required when --executor coding".to_string())?;
            let agent_program = agent_program
                .ok_or_else(|| "--agent-command is required when --executor coding".to_string())?;
            let agent = agent_surface(&agent_program, agent_args)?;

            Ok(ExecutorSelection::Coding(CodingSurface {
                workspace_root,
                git_base_url,
                agent,
            }))
        }
    }
}

/// Builds the [`AgentSurface`] for the `--agent-command` program and its
/// trailing `--agent-arg` values. The `anvil-native` program name parses the agent flags
/// in-process; any other program is spawned as an external command.
fn agent_surface(program: &str, args: Vec<String>) -> Result<AgentSurface, String> {
    if program == ANVIL_NATIVE_PROGRAM {
        Ok(AgentSurface::AnvilNative(parse_anvil_native_agent_surface(
            args,
        )?))
    } else {
        let mut command = Vec::with_capacity(args.len() + 1);
        command.push(program.to_string());
        command.extend(args);
        Ok(AgentSurface::ExternalCommand(command))
    }
}

/// Parses the anvil-native agent flags from the `--agent-arg` values — the
/// flags the `anvil-agent` binary parses for itself.
fn parse_anvil_native_agent_surface(args: Vec<String>) -> Result<AnvilNativeAgentSurface, String> {
    let mut agent_program = ANVIL_AGENT_PROGRAM.to_string();
    let mut auth = AgentAuthChoice::ChatGptOAuth;
    let mut codex_model = None;
    let mut auth_file = None;
    let mut config_dir = None;
    let mut max_iterations = None;
    let mut enable_subagents = false;

    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--enable-subagents" => {
                enable_subagents = true;
            }
            "--agent-program" => {
                agent_program = iter
                    .next()
                    .ok_or_else(|| "--agent-program requires a value".to_string())?;
            }
            "--auth" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--auth requires a value".to_string())?;
                auth = parse_agent_auth(&value)?;
            }
            "--codex-model" => {
                codex_model = Some(
                    iter.next()
                        .ok_or_else(|| "--codex-model requires a value".to_string())?,
                );
            }
            "--auth-file" => {
                auth_file = Some(PathBuf::from(
                    iter.next()
                        .ok_or_else(|| "--auth-file requires a value".to_string())?,
                ));
            }
            "--config-dir" => {
                config_dir =
                    Some(PathBuf::from(iter.next().ok_or_else(|| {
                        "--config-dir requires a value".to_string()
                    })?));
            }
            "--max-iterations" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--max-iterations requires a value".to_string())?;
                let parsed: usize = value.parse().map_err(|error| {
                    format!("--max-iterations must be a positive integer: {error}")
                })?;
                if parsed == 0 {
                    return Err("--max-iterations must be greater than zero".to_string());
                }
                max_iterations = Some(parsed);
            }
            other => {
                return Err(format!(
                    "unknown anvil-native agent arg `{other}`; expected --agent-program, --auth, --codex-model, --auth-file, --config-dir, --max-iterations, or --enable-subagents"
                ));
            }
        }
    }

    Ok(AnvilNativeAgentSurface {
        agent_program,
        auth,
        codex_model,
        auth_file,
        config_dir,
        max_iterations,
        enable_subagents,
    })
}

fn parse_agent_auth(value: &str) -> Result<AgentAuthChoice, String> {
    match value {
        "deepseek" => Ok(AgentAuthChoice::DeepSeek),
        "chatgpt-oauth" => Ok(AgentAuthChoice::ChatGptOAuth),
        "anthropic-oauth" => Ok(AgentAuthChoice::AnthropicOAuth),
        other => Err(format!(
            "unsupported --auth `{other}`; expected deepseek, chatgpt-oauth, or anthropic-oauth"
        )),
    }
}

pub fn role_identities_from_env(
    roles: impl IntoIterator<Item = String>,
    vars: impl IntoIterator<Item = (String, String)>,
) -> Result<BTreeMap<String, RoleGitIdentity>, String> {
    let roles: BTreeSet<String> = roles.into_iter().collect();
    let vars: BTreeMap<String, String> = vars.into_iter().collect();
    let mut identities = BTreeMap::new();

    for role in roles {
        let key = env_role_key(&role);
        let user_var = format!("TEMPER_FORGEJO_USER_{key}");
        let token_var = format!("TEMPER_FORGEJO_TOKEN_{key}");
        let email_var = format!("TEMPER_FORGEJO_EMAIL_{key}");

        let user = required_env_value(&vars, &user_var, &role)?;
        let token = required_env_value(&vars, &token_var, &role)?;
        let email = vars
            .get(&email_var)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("{user}@noreply.localhost"));

        identities.insert(role, RoleGitIdentity { user, email, token });
    }

    Ok(identities)
}

fn env_role_key(role: &str) -> String {
    role.chars()
        .flat_map(char::to_uppercase)
        .map(|character| {
            if character.is_ascii_uppercase() || character.is_ascii_digit() {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn required_env_value(
    vars: &BTreeMap<String, String>,
    var_name: &str,
    role: &str,
) -> Result<String, String> {
    vars.get(var_name)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            format!("no {var_name} in the environment for role `{role}`; is roles.env provisioned?")
        })
}

fn parse_capability(value: &str) -> Result<CapabilitySpec, String> {
    let mut parts = value.splitn(2, ':');
    let repo = parts
        .next()
        .expect("splitn always returns the first part")
        .trim();
    let role = parts
        .next()
        .ok_or_else(|| format!("invalid --capability `{value}`; expected <owner/name>:<role>"))?
        .trim();

    validate_repo(repo).map_err(|message| format!("invalid --capability `{value}`: {message}"))?;
    if role.is_empty() {
        return Err(format!(
            "invalid --capability `{value}`: role must not be empty"
        ));
    }

    Ok(CapabilitySpec {
        repo: repo.to_string(),
        role: role.to_string(),
    })
}

fn validate_repo(repo: &str) -> Result<(), String> {
    let mut parts = repo.split('/');
    let owner = parts.next().unwrap_or_default();
    let name = parts.next().unwrap_or_default();
    if owner.is_empty() || name.is_empty() || parts.next().is_some() {
        return Err("repo must be owner/name with exactly two non-empty parts".to_string());
    }
    Ok(())
}

fn parse_non_zero_u32(flag: &str, value: &str) -> Result<u32, String> {
    let parsed: u32 = value
        .trim()
        .parse()
        .map_err(|error| format!("{flag} must be a positive integer: {error}"))?;
    if parsed == 0 {
        return Err(format!("{flag} must be greater than zero"));
    }
    Ok(parsed)
}

fn parse_non_zero_u64(flag: &str, value: &str) -> Result<u64, String> {
    let parsed: u64 = value
        .trim()
        .parse()
        .map_err(|error| format!("{flag} must be a positive integer: {error}"))?;
    if parsed == 0 {
        return Err(format!("{flag} must be greater than zero"));
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(args: &[&str]) -> WorkerConfig {
        match parse(args.iter().map(|arg| (*arg).to_string())).expect("parse succeeds") {
            ParseOutcome::Run(config) => config,
            ParseOutcome::Help => panic!("expected run config"),
        }
    }

    fn parse_err(args: &[&str]) -> String {
        parse(args.iter().map(|arg| (*arg).to_string())).expect_err("parse fails")
    }

    #[test]
    fn parses_defaults_and_trims_required_values() {
        let config = parse_ok(&[
            "--daemon-url",
            " https://temper.example/ ",
            "--worker-id",
            " worker-1 ",
            "--capability",
            " ai/temper : coder ",
        ]);

        assert_eq!(config.daemon_url, "https://temper.example/");
        assert_eq!(config.worker_id, "worker-1");
        assert_eq!(
            config.capabilities,
            vec![CapabilitySpec {
                repo: "ai/temper".to_string(),
                role: "coder".to_string(),
            }]
        );
        assert_eq!(config.max_concurrent_jobs, 1);
        assert_eq!(config.poll_wait, Duration::from_millis(30_000));
        assert_eq!(config.heartbeat_interval, Duration::from_millis(10_000));
        assert_eq!(config.executor, ExecutorSelection::Stub);
    }

    #[test]
    fn parses_anvil_native_agent_surface_from_agent_args() {
        let config = parse_ok(&[
            "--daemon-url",
            "http://daemon.example",
            "--worker-id",
            "worker-1",
            "--capability",
            "ai/temper:coder",
            "--executor",
            "coding",
            "--workspace-root",
            " /var/lib/smith/workspaces ",
            "--git-base-url",
            " https://forgejo.example ",
            "--agent-command",
            " anvil-native ",
            "--agent-arg",
            "--auth",
            "--agent-arg",
            "anthropic-oauth",
            "--agent-arg",
            "--auth-file",
            "--agent-arg",
            "/tmp/auth.json",
            "--agent-arg",
            "--max-iterations",
            "--agent-arg",
            "42",
        ]);

        assert_eq!(
            config.executor,
            ExecutorSelection::Coding(CodingSurface {
                workspace_root: PathBuf::from("/var/lib/smith/workspaces"),
                git_base_url: "https://forgejo.example".to_string(),
                agent: AgentSurface::AnvilNative(AnvilNativeAgentSurface {
                    agent_program: "anvil-agent".to_string(),
                    auth: AgentAuthChoice::AnthropicOAuth,
                    codex_model: None,
                    auth_file: Some(PathBuf::from("/tmp/auth.json")),
                    config_dir: None,
                    max_iterations: Some(42),
                    enable_subagents: false,
                }),
            })
        );
    }

    #[test]
    fn anvil_native_enable_subagents_flag_is_parsed() {
        let config = parse_ok(&[
            "--daemon-url",
            "http://daemon.example",
            "--worker-id",
            "worker-1",
            "--capability",
            "ai/temper:coder",
            "--executor",
            "coding",
            "--workspace-root",
            "/workspaces",
            "--git-base-url",
            "https://forgejo.example",
            "--agent-command",
            "anvil-native",
            "--agent-arg",
            "--enable-subagents",
        ]);
        let ExecutorSelection::Coding(surface) = config.executor else {
            panic!("expected coding executor");
        };
        let AgentSurface::AnvilNative(native) = surface.agent else {
            panic!("expected anvil-native agent");
        };
        assert!(native.enable_subagents);
    }

    #[test]
    fn anvil_native_defaults_to_chatgpt_oauth_when_no_args() {
        let config = parse_ok(&[
            "--daemon-url",
            "http://daemon.example",
            "--worker-id",
            "worker-1",
            "--capability",
            "ai/temper:coder",
            "--executor",
            "coding",
            "--workspace-root",
            "/workspaces",
            "--git-base-url",
            "https://forgejo.example",
            "--agent-command",
            "anvil-native",
        ]);

        let ExecutorSelection::Coding(surface) = config.executor else {
            panic!("expected coding executor");
        };
        assert_eq!(
            surface.agent,
            AgentSurface::AnvilNative(AnvilNativeAgentSurface {
                agent_program: "anvil-agent".to_string(),
                auth: AgentAuthChoice::ChatGptOAuth,
                codex_model: None,
                auth_file: None,
                config_dir: None,
                max_iterations: None,
                enable_subagents: false,
            })
        );
    }

    #[test]
    fn non_native_agent_command_is_external_passthrough() {
        let config = parse_ok(&[
            "--daemon-url",
            "http://daemon.example",
            "--worker-id",
            "worker-1",
            "--capability",
            "ai/temper:coder",
            "--executor",
            "coding",
            "--workspace-root",
            "/workspaces",
            "--git-base-url",
            "https://forgejo.example",
            "--agent-command",
            "/opt/greeting-coder.sh",
            "--agent-arg",
            "--verbose",
        ]);

        let ExecutorSelection::Coding(surface) = config.executor else {
            panic!("expected coding executor");
        };
        assert_eq!(
            surface.agent,
            AgentSurface::ExternalCommand(vec![
                "/opt/greeting-coder.sh".to_string(),
                "--verbose".to_string(),
            ])
        );
    }

    #[test]
    fn anvil_native_rejects_unknown_arg() {
        let error = parse_err(&[
            "--daemon-url",
            "http://daemon.example",
            "--worker-id",
            "worker-1",
            "--capability",
            "ai/temper:coder",
            "--executor",
            "coding",
            "--workspace-root",
            "/workspaces",
            "--git-base-url",
            "https://forgejo.example",
            "--agent-command",
            "anvil-native",
            "--agent-arg",
            "--verbose",
        ]);
        assert!(
            error.contains("unknown anvil-native agent arg `--verbose`"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn coding_executor_requires_all_coding_flags() {
        for missing_flag in ["--workspace-root", "--git-base-url", "--agent-command"] {
            let mut args = vec![
                "--daemon-url",
                "http://daemon.example",
                "--worker-id",
                "worker-1",
                "--capability",
                "ai/temper:coder",
                "--executor",
                "coding",
            ];
            if missing_flag != "--workspace-root" {
                args.extend(["--workspace-root", "/workspaces"]);
            }
            if missing_flag != "--git-base-url" {
                args.extend(["--git-base-url", "https://forgejo.example"]);
            }
            if missing_flag != "--agent-command" {
                args.extend(["--agent-command", "anvil-native"]);
            }

            let error = parse_err(&args);
            assert!(
                error.contains(missing_flag),
                "unexpected error for missing {missing_flag}: {error}"
            );
        }
    }

    #[test]
    fn rejects_bogus_executor_and_coding_flags_with_stub_executor() {
        assert!(
            parse_err(&[
                "--daemon-url",
                "http://daemon.example",
                "--worker-id",
                "worker-1",
                "--capability",
                "ai/temper:coder",
                "--executor",
                "bogus",
            ])
            .contains("--executor must be")
        );

        for flag in [
            "--workspace-root",
            "--git-base-url",
            "--agent-command",
            "--agent-arg",
        ] {
            let error = parse_err(&[
                "--daemon-url",
                "http://daemon.example",
                "--worker-id",
                "worker-1",
                "--capability",
                "ai/temper:coder",
                flag,
                "value",
            ]);
            assert!(
                error.contains(&format!("{flag} requires --executor coding")),
                "unexpected error for {flag}: {error}"
            );
        }
    }

    #[test]
    fn singleton_flags_use_last_value_and_numeric_overrides() {
        let config = parse_ok(&[
            "--daemon-url",
            "http://old.example",
            "--daemon-url",
            "http://new.example",
            "--worker-id",
            "old-worker",
            "--worker-id",
            "new-worker",
            "--capability",
            "ai/temper:coder",
            "--max-concurrent",
            "2",
            "--poll-wait-ms",
            "500",
            "--heartbeat-interval-ms",
            "250",
        ]);

        assert_eq!(config.daemon_url, "http://new.example");
        assert_eq!(config.worker_id, "new-worker");
        assert_eq!(config.max_concurrent_jobs, 2);
        assert_eq!(config.poll_wait, Duration::from_millis(500));
        assert_eq!(config.heartbeat_interval, Duration::from_millis(250));
    }

    #[test]
    fn repeated_capabilities_are_deduplicated_preserving_order() {
        let config = parse_ok(&[
            "--daemon-url",
            "http://daemon.example",
            "--worker-id",
            "worker-1",
            "--capability",
            "ai/temper:coder",
            "--capability",
            " ai/temper : coder ",
            "--capability",
            "ai/smith:engineer",
            "--capability",
            "ai/temper:architect",
        ]);

        assert_eq!(
            config.capabilities,
            vec![
                CapabilitySpec {
                    repo: "ai/temper".to_string(),
                    role: "coder".to_string(),
                },
                CapabilitySpec {
                    repo: "ai/smith".to_string(),
                    role: "engineer".to_string(),
                },
                CapabilitySpec {
                    repo: "ai/temper".to_string(),
                    role: "architect".to_string(),
                },
            ]
        );
    }

    #[test]
    fn rejects_malformed_capabilities() {
        for capability in ["nope", "ai/temper", ":role", "ai/temper:"] {
            let error = parse_err(&[
                "--daemon-url",
                "http://daemon.example",
                "--worker-id",
                "worker-1",
                "--capability",
                capability,
            ]);
            assert!(
                error.contains("invalid --capability"),
                "unexpected error for {capability:?}: {error}"
            );
        }
    }

    #[test]
    fn rejects_missing_required_flags() {
        assert!(parse_err(&[]).contains("--daemon-url is required"));
        assert!(
            parse_err(&["--daemon-url", "http://daemon.example"])
                .contains("--worker-id is required")
        );
        assert!(
            parse_err(&[
                "--daemon-url",
                "http://daemon.example",
                "--worker-id",
                "worker-1",
            ])
            .contains("--capability is required")
        );
        assert!(
            parse_err(&[
                "--daemon-url",
                " ",
                "--worker-id",
                "worker-1",
                "--capability",
                "ai/temper:coder",
            ])
            .contains("--daemon-url must not be empty")
        );
    }

    #[test]
    fn rejects_zero_and_invalid_numerics() {
        for (flag, value) in [
            ("--max-concurrent", "0"),
            ("--max-concurrent", "nope"),
            ("--poll-wait-ms", "0"),
            ("--poll-wait-ms", "1.5"),
            ("--heartbeat-interval-ms", "0"),
            ("--heartbeat-interval-ms", "NaN"),
        ] {
            let error = parse_err(&[
                "--daemon-url",
                "http://daemon.example",
                "--worker-id",
                "worker-1",
                "--capability",
                "ai/temper:coder",
                flag,
                value,
            ]);
            assert!(error.contains(flag), "unexpected error for {flag}: {error}");
        }
    }

    #[test]
    fn rejects_unknown_flags_positionals_and_missing_values() {
        assert!(
            parse_err(&[
                "--daemon-url",
                "http://daemon.example",
                "--worker-id",
                "worker-1",
                "--capability",
                "ai/temper:coder",
                "--unknown",
            ])
            .contains("unknown flag")
        );
        assert!(parse_err(&["positional"]).contains("unexpected positional argument"));
        assert!(parse_err(&["--daemon-url"]).contains("--daemon-url requires a value"));
        assert!(
            parse_err(&["--daemon-url", "--worker-id"]).contains("--daemon-url requires a value")
        );
    }

    #[test]
    fn help_anywhere_returns_help_before_validation() {
        assert_eq!(
            parse(["--help".to_string()]).expect("help parses"),
            ParseOutcome::Help
        );
        assert_eq!(
            parse(["--daemon-url".to_string(), "-h".to_string()]).expect("help parses"),
            ParseOutcome::Help
        );
        assert!(
            parse([
                "--executor".to_string(),
                "coding".to_string(),
                "--agent-arg".to_string(),
                "--help".to_string(),
            ])
            .expect_err("agent arg help-looking value is not global help")
            .contains("--daemon-url is required")
        );
    }

    #[test]
    fn loads_role_identities_from_env_for_distinct_roles() {
        let identities = role_identities_from_env(
            [
                "engineer".to_string(),
                "code-reviewer".to_string(),
                "engineer".to_string(),
            ],
            [
                (
                    "TEMPER_FORGEJO_USER_ENGINEER".to_string(),
                    " engineer-user ".to_string(),
                ),
                (
                    "TEMPER_FORGEJO_TOKEN_ENGINEER".to_string(),
                    " engineer-token ".to_string(),
                ),
                (
                    "TEMPER_FORGEJO_USER_CODE_REVIEWER".to_string(),
                    "reviewer-user".to_string(),
                ),
                (
                    "TEMPER_FORGEJO_TOKEN_CODE_REVIEWER".to_string(),
                    "reviewer-token".to_string(),
                ),
                (
                    "TEMPER_FORGEJO_EMAIL_CODE_REVIEWER".to_string(),
                    "reviewer@example.test".to_string(),
                ),
                (
                    "TEMPER_FORGEJO_USER_ARCHITECT".to_string(),
                    "ignored".to_string(),
                ),
                (
                    "TEMPER_FORGEJO_TOKEN_ARCHITECT".to_string(),
                    "ignored".to_string(),
                ),
            ],
        )
        .expect("identities load");

        assert_eq!(identities.len(), 2);
        assert_eq!(
            identities.get("engineer"),
            Some(&RoleGitIdentity {
                user: "engineer-user".to_string(),
                email: "engineer-user@noreply.localhost".to_string(),
                token: "engineer-token".to_string(),
            })
        );
        assert_eq!(
            identities.get("code-reviewer"),
            Some(&RoleGitIdentity {
                user: "reviewer-user".to_string(),
                email: "reviewer@example.test".to_string(),
                token: "reviewer-token".to_string(),
            })
        );
        assert!(!identities.contains_key("architect"));
    }

    #[test]
    fn role_identity_errors_name_missing_user_or_token_and_role() {
        let missing_user = role_identities_from_env(
            ["engineer".to_string()],
            [(
                "TEMPER_FORGEJO_TOKEN_ENGINEER".to_string(),
                "token".to_string(),
            )],
        )
        .expect_err("missing user fails");
        assert!(missing_user.contains("TEMPER_FORGEJO_USER_ENGINEER"));
        assert!(missing_user.contains("role `engineer`"));

        let missing_token = role_identities_from_env(
            ["code-reviewer".to_string()],
            [
                (
                    "TEMPER_FORGEJO_USER_CODE_REVIEWER".to_string(),
                    "reviewer".to_string(),
                ),
                (
                    "TEMPER_FORGEJO_TOKEN_CODE_REVIEWER".to_string(),
                    " ".to_string(),
                ),
            ],
        )
        .expect_err("missing token fails");
        assert!(missing_token.contains("TEMPER_FORGEJO_TOKEN_CODE_REVIEWER"));
        assert!(missing_token.contains("role `code-reviewer`"));
    }
}
