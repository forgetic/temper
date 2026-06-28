use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use super::agent::agent_surface;
use super::types::{CapabilitySpec, CodingSurface, ExecutorSelection, ParseOutcome, WorkerConfig};

pub const USAGE: &str = "smith-worker --daemon-url <url> --worker-id <id> --capability <owner/name>:<role> [--capability ...] [--max-concurrent <n>] [--poll-wait-ms <n>] [--heartbeat-interval-ms <n>] [--executor <stub|coding>] [--workspace-root <path>] [--git-base-url <url>] [--agent-command <anvil-native|program>] [--agent-arg <arg> ...]\n  --agent-command anvil-native spawns the out-of-process temper-agent; its --agent-arg values (--agent-program, --provider, --model, --capture-dir, --max-iterations, --subagents) become the agent's flags. Any other --agent-command is spawned verbatim over the same protocol.";

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
                daemon_url = Some(required_trimmed_value("--daemon-url", value)?);
            }
            "--worker-id" => {
                let value = flag_value(&args, &mut index, "--worker-id")?;
                worker_id = Some(required_trimmed_value("--worker-id", value)?);
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
        // The CLI surface carries no identities; the `smith-worker` binary fills
        // them from the environment for the coding executor (the env reads stay in
        // the binary, not the library).
        role_identities: std::collections::BTreeMap::new(),
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
            stub_executor_selection(workspace_root, git_base_url, agent_program, agent_args)
        }
        ExecutorKind::Coding => {
            coding_executor_selection(workspace_root, git_base_url, agent_program, agent_args)
        }
    }
}

fn stub_executor_selection(
    workspace_root: Option<PathBuf>,
    git_base_url: Option<String>,
    agent_program: Option<String>,
    agent_args: Vec<String>,
) -> Result<ExecutorSelection, String> {
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

fn coding_executor_selection(
    workspace_root: Option<PathBuf>,
    git_base_url: Option<String>,
    agent_program: Option<String>,
    agent_args: Vec<String>,
) -> Result<ExecutorSelection, String> {
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
