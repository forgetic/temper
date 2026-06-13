//! Argument parsing for `temper-worker`.

use chrono::Duration;
use std::collections::BTreeSet;
use std::fmt;
use std::path::PathBuf;
use std::time::Duration as StdDuration;
use temper_forge::RepositoryPath;
use temper_runner::WorkflowRoleDecisionProcessConfig;

pub const WORKFLOW_FILE_ENV: &str = "TEMPER_WORKFLOW_FILE";
pub const FORGEJO_TOKEN_ENV: &str = "TEMPER_FORGEJO_TOKEN";
pub const FORGEJO_USERNAME_ENV: &str = "TEMPER_FORGEJO_USERNAME";
pub const FORGEJO_PASSWORD_ENV: &str = "TEMPER_FORGEJO_PASSWORD";
pub const ROLE_DECISION_COMMAND_ENV: &str = "TEMPER_WORKER_ROLE_DECISION_COMMAND";
pub const ROLE_DECISION_ARGS_ENV: &str = "TEMPER_WORKER_ROLE_DECISION_ARGS_JSON";
pub const ROLE_DECISION_CWD_ENV: &str = "TEMPER_WORKER_ROLE_DECISION_CWD";
pub const ROLE_DECISION_ENV_ALLOWLIST_ENV: &str = "TEMPER_WORKER_ROLE_DECISION_ENV_ALLOWLIST";
pub const ROLE_DECISION_TIMEOUT_ENV: &str = "TEMPER_WORKER_ROLE_DECISION_TIMEOUT_SECS";
pub const DEFAULT_AUDIT_INTERVAL_MS: i64 = 600_000;
pub const DEFAULT_IDLE_POLL_MAX_MS: i64 = 60_000;

pub const USAGE: &str = concat!(
    "temper-worker --backend forgejo --base-url <url> ",
    "(--repo <owner/name> [--repo <owner/name> ...] | --repo-list <path>) ",
    "--kind <role|mechanical> [--role <id> --user <handle>] ",
    "[--workflow <path>] ",
    "[--role-decision-command <path>] [--role-decision-arg <arg>] ",
    "[--role-decision-env <name>] [--role-decision-cwd <path>] ",
    "[--role-decision-timeout-secs <n>] ",
    "[--poll-ms <n>] [--idle-poll-max-ms <n mechanical idle cap>] ",
    "[--audit-ms <n deep-audit, 0 disables>] ",
    "[--stop-file <path>] [--run-secs <max>] [--wake-socket <path>] ",
    "[--wake-secret-file <path>] [--allow-bookkeeping-only-pr]\n",
    "  forgejo token comes from TEMPER_FORGEJO_TOKEN; optional web UI credentials ",
    "come from TEMPER_FORGEJO_USERNAME/TEMPER_FORGEJO_PASSWORD; role ",
    "decision process config comes from TEMPER_WORKER_ROLE_DECISION_*; ",
    "the workflow file may also come from TEMPER_WORKFLOW_FILE, defaulting to ",
    "the bundled reference-delivery workflow when unset"
);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseOutcome {
    Run(Box<WorkerArgs>),
    Help,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkerKind {
    Role { role: String, user: String },
    Mechanical,
}

#[derive(Clone, Eq, PartialEq)]
pub struct ForgejoArgs {
    pub base_url: String,
    pub token: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl fmt::Debug for ForgejoArgs {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ForgejoArgs")
            .field("base_url", &self.base_url)
            .field("token", &"<redacted>")
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct WorkerArgs {
    pub kind: WorkerKind,
    pub forgejo: ForgejoArgs,
    /// First configured repository, retained for legacy callers/tests.
    pub owner: String,
    /// First configured repository, retained for legacy callers/tests.
    pub name: String,
    /// Repositories this worker scans. This is a scan shard, not a write-authority list:
    /// Forge permissions on the token remain the authority for mutations.
    pub repositories: Vec<RepositoryPath>,
    pub poll_interval: Duration,
    /// Maximum mechanical poll cadence after repeated successful no-action normal ticks.
    pub idle_poll_max_interval: Duration,
    /// Low-frequency broad audit cadence. `None` disables audit ticks.
    pub audit_interval: Option<Duration>,
    pub stop_file: Option<PathBuf>,
    pub run_secs: Option<u64>,
    pub wake_socket: Option<PathBuf>,
    pub wake_secret_file: Option<PathBuf>,
    pub role_decision_process: Option<WorkflowRoleDecisionProcessConfig>,
    /// Allows guarded role agents to approve PRs whose changed files are only
    /// Temper bookkeeping paths. Intended only with synthetic demos.
    pub allow_bookkeeping_only_pr: bool,
    /// Workflow document to operate against. `None` uses the bundled
    /// reference-delivery workflow, reproducing today's default behavior.
    pub workflow_file: Option<PathBuf>,
}

impl fmt::Debug for WorkerArgs {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkerArgs")
            .field("kind", &self.kind)
            .field("forgejo", &self.forgejo)
            .field("owner", &self.owner)
            .field("name", &self.name)
            .field("repositories", &self.repositories)
            .field("poll_interval", &self.poll_interval)
            .field("idle_poll_max_interval", &self.idle_poll_max_interval)
            .field("audit_interval", &self.audit_interval)
            .field("stop_file", &self.stop_file)
            .field("run_secs", &self.run_secs)
            .field("wake_socket", &self.wake_socket)
            .field("wake_secret_file", &self.wake_secret_file)
            .field(
                "role_decision_process",
                &self.role_decision_process.as_ref().map(|_| "<configured>"),
            )
            .field("allow_bookkeeping_only_pr", &self.allow_bookkeeping_only_pr)
            .field("workflow_file", &self.workflow_file)
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArgsError(String);

impl ArgsError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ArgsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ArgsError {}

pub fn parse<I>(args: I) -> Result<ParseOutcome, ArgsError>
where
    I: IntoIterator<Item = String>,
{
    parse_with_env(args, |key| std::env::var(key).ok())
}

pub fn parse_with_env<I, E>(args: I, env: E) -> Result<ParseOutcome, ArgsError>
where
    I: IntoIterator<Item = String>,
    E: Fn(&str) -> Option<String>,
{
    let raw = RawArgs::collect(args)?;
    if raw.help {
        return Ok(ParseOutcome::Help);
    }
    raw.into_worker_args(&env)
        .map(|args| ParseOutcome::Run(Box::new(args)))
}

#[derive(Default)]
struct RawArgs {
    help: bool,
    backend: Option<String>,
    base_url: Option<String>,
    repos: Vec<String>,
    repo_list: Option<String>,
    kind: Option<String>,
    role: Option<String>,
    user: Option<String>,
    poll_ms: Option<String>,
    idle_poll_max_ms: Option<String>,
    audit_ms: Option<String>,
    stop_file: Option<String>,
    run_secs: Option<String>,
    wake_socket: Option<String>,
    wake_secret_file: Option<String>,
    role_decision: RawRoleDecisionProcessArgs,
    allow_bookkeeping_only_pr: bool,
    workflow_file: Option<String>,
}

impl RawArgs {
    fn collect<I>(args: I) -> Result<Self, ArgsError>
    where
        I: IntoIterator<Item = String>,
    {
        let mut raw = RawArgs::default();
        let mut iter = args.into_iter();
        while let Some(flag) = iter.next() {
            match flag.as_str() {
                "--help" | "-h" => raw.help = true,
                "--backend" => raw.backend = Some(value_for(&flag, &mut iter)?),
                "--base-url" => raw.base_url = Some(value_for(&flag, &mut iter)?),
                "--repo" => raw.repos.push(value_for(&flag, &mut iter)?),
                "--repo-list" => raw.repo_list = Some(value_for(&flag, &mut iter)?),
                "--kind" => raw.kind = Some(value_for(&flag, &mut iter)?),
                "--role" => raw.role = Some(value_for(&flag, &mut iter)?),
                "--user" => raw.user = Some(value_for(&flag, &mut iter)?),
                "--poll-ms" => raw.poll_ms = Some(value_for(&flag, &mut iter)?),
                "--idle-poll-max-ms" => raw.idle_poll_max_ms = Some(value_for(&flag, &mut iter)?),
                "--audit-ms" => raw.audit_ms = Some(value_for(&flag, &mut iter)?),
                "--stop-file" => raw.stop_file = Some(value_for(&flag, &mut iter)?),
                "--run-secs" => raw.run_secs = Some(value_for(&flag, &mut iter)?),
                "--wake-socket" => raw.wake_socket = Some(value_for(&flag, &mut iter)?),
                "--wake-secret-file" => raw.wake_secret_file = Some(value_for(&flag, &mut iter)?),
                "--role-decision-command" => {
                    raw.role_decision.command = Some(value_for(&flag, &mut iter)?)
                }
                "--role-decision-arg" => raw.role_decision.args.push(value_for(&flag, &mut iter)?),
                "--role-decision-cwd" => raw.role_decision.cwd = Some(value_for(&flag, &mut iter)?),
                "--role-decision-env" => raw
                    .role_decision
                    .env_allowlist
                    .push(value_for(&flag, &mut iter)?),
                "--role-decision-timeout-secs" => {
                    raw.role_decision.timeout_secs = Some(value_for(&flag, &mut iter)?)
                }
                "--workflow" => raw.workflow_file = Some(value_for(&flag, &mut iter)?),
                "--allow-bookkeeping-only-pr" => raw.allow_bookkeeping_only_pr = true,
                other => {
                    return Err(ArgsError::new(format!(
                        "unrecognized argument '{other}'\nusage: {USAGE}"
                    )))
                }
            }
        }
        Ok(raw)
    }

    fn into_worker_args<E>(self, env: &E) -> Result<WorkerArgs, ArgsError>
    where
        E: Fn(&str) -> Option<String>,
    {
        match self.backend.as_deref() {
            Some("forgejo") => {}
            Some(other) => {
                return Err(ArgsError::new(format!(
                    "unknown --backend '{other}'; expected forgejo"
                )))
            }
            None => {
                return Err(ArgsError::new(format!(
                    "missing required --backend forgejo\nusage: {USAGE}"
                )))
            }
        }
        let kind = self.parse_kind()?;
        let repositories = parse_repositories(self.repos, self.repo_list)?;
        let first = repositories
            .first()
            .expect("parse_repositories returns at least one repository");
        let owner = first.owner.clone();
        let name = first.name.clone();
        let token = require_env(env, FORGEJO_TOKEN_ENV)?;
        let forgejo = ForgejoArgs {
            base_url: require(self.base_url, "--base-url")?,
            token,
            username: non_empty_env(env, FORGEJO_USERNAME_ENV),
            password: non_empty_env(env, FORGEJO_PASSWORD_ENV),
        };
        let role_decision_process = if matches!(kind, WorkerKind::Role { .. }) {
            let process = self.role_decision.into_config(env)?;
            if process.is_none() {
                return Err(ArgsError::new(concat!(
                    "role workers require --role-decision-command or ",
                    "TEMPER_WORKER_ROLE_DECISION_COMMAND; Smith provides the first ",
                    "concrete responder"
                )));
            }
            process
        } else {
            None
        };
        let poll_interval = Duration::milliseconds(match self.poll_ms {
            Some(raw) => parse_i64(&raw, "--poll-ms")?,
            None => 1_000,
        });
        let idle_poll_max_interval =
            parse_idle_poll_max_interval(self.idle_poll_max_ms, poll_interval)?;
        Ok(WorkerArgs {
            kind,
            forgejo,
            owner,
            name,
            repositories,
            poll_interval,
            idle_poll_max_interval,
            audit_interval: parse_audit_interval(self.audit_ms)?,
            stop_file: self.stop_file.map(PathBuf::from),
            run_secs: self
                .run_secs
                .map(|raw| parse_u64(&raw, "--run-secs"))
                .transpose()?,
            wake_socket: non_empty(self.wake_socket).map(PathBuf::from),
            wake_secret_file: non_empty(self.wake_secret_file).map(PathBuf::from),
            role_decision_process,
            allow_bookkeeping_only_pr: self.allow_bookkeeping_only_pr,
            workflow_file: non_empty(self.workflow_file)
                .or_else(|| non_empty_env(env, WORKFLOW_FILE_ENV))
                .map(PathBuf::from),
        })
    }

    fn parse_kind(&self) -> Result<WorkerKind, ArgsError> {
        match require_ref(self.kind.as_deref(), "--kind")?.as_str() {
            "mechanical" => Ok(WorkerKind::Mechanical),
            "role" => Ok(WorkerKind::Role {
                role: require_ref(self.role.as_deref(), "--role (required for --kind role)")?,
                user: require_ref(self.user.as_deref(), "--user (required for --kind role)")?,
            }),
            other => Err(ArgsError::new(format!(
                "unknown --kind '{other}'; expected role|mechanical"
            ))),
        }
    }
}

#[derive(Default)]
struct RawRoleDecisionProcessArgs {
    command: Option<String>,
    args: Vec<String>,
    cwd: Option<String>,
    env_allowlist: Vec<String>,
    timeout_secs: Option<String>,
}

impl RawRoleDecisionProcessArgs {
    fn into_config<E>(self, env: &E) -> Result<Option<WorkflowRoleDecisionProcessConfig>, ArgsError>
    where
        E: Fn(&str) -> Option<String>,
    {
        let Some(command) =
            non_empty(self.command).or_else(|| non_empty_env(env, ROLE_DECISION_COMMAND_ENV))
        else {
            return Ok(None);
        };
        let args = if self.args.is_empty() {
            parse_role_decision_args_json(non_empty_env(env, ROLE_DECISION_ARGS_ENV))?
        } else {
            self.args
        };
        let cwd = non_empty(self.cwd).or_else(|| non_empty_env(env, ROLE_DECISION_CWD_ENV));
        let env_allowlist = if self.env_allowlist.is_empty() {
            parse_env_allowlist(non_empty_env(env, ROLE_DECISION_ENV_ALLOWLIST_ENV))
        } else {
            self.env_allowlist
        };
        // Resolve the allowlisted names to values here, at the one boundary that
        // reads process environment. The subprocess adapter forwards these
        // resolved pairs verbatim and never touches ambient environment itself.
        // A name with no value present (or empty) is dropped, matching the old
        // skip-on-absent behavior.
        let env_pairs: Vec<(String, String)> = env_allowlist
            .into_iter()
            .filter_map(|name| env(&name).map(|value| (name, value)))
            .collect();
        let timeout_secs = match self
            .timeout_secs
            .or_else(|| non_empty_env(env, ROLE_DECISION_TIMEOUT_ENV))
        {
            Some(raw) => parse_role_decision_timeout_secs(&raw)?,
            None => WorkflowRoleDecisionProcessConfig::DEFAULT_TIMEOUT.as_secs(),
        };
        let mut config = WorkflowRoleDecisionProcessConfig::new(command)
            .with_args(args)
            .with_env(env_pairs)
            .with_timeout(StdDuration::from_secs(timeout_secs));
        if let Some(cwd) = cwd {
            config = config.with_working_dir(cwd);
        }
        Ok(Some(config))
    }
}

fn value_for<I>(flag: &str, iter: &mut I) -> Result<String, ArgsError>
where
    I: Iterator<Item = String>,
{
    iter.next()
        .ok_or_else(|| ArgsError::new(format!("flag '{flag}' expects a value")))
}

fn require(value: Option<String>, flag: &str) -> Result<String, ArgsError> {
    value.ok_or_else(|| ArgsError::new(format!("missing required {flag}\nusage: {USAGE}")))
}

fn require_ref(value: Option<&str>, flag: &str) -> Result<String, ArgsError> {
    value
        .map(str::to_string)
        .ok_or_else(|| ArgsError::new(format!("missing required {flag}\nusage: {USAGE}")))
}

fn parse_repositories(
    repos: Vec<String>,
    repo_list: Option<String>,
) -> Result<Vec<RepositoryPath>, ArgsError> {
    let mut raw = repos;
    if let Some(path) = repo_list {
        let contents = std::fs::read_to_string(&path).map_err(|error| {
            ArgsError::new(format!("failed to read --repo-list {path}: {error}"))
        })?;
        for (index, line) in contents.lines().enumerate() {
            let trimmed = line.split('#').next().unwrap_or_default().trim();
            if !trimmed.is_empty() {
                raw.push(trimmed.to_string());
            } else if line.trim_start().starts_with('#') || line.trim().is_empty() {
                continue;
            } else {
                return Err(ArgsError::new(format!(
                    "malformed --repo-list {path} line {}",
                    index + 1
                )));
            }
        }
    }
    if raw.is_empty() {
        return Err(ArgsError::new(format!(
            "missing required --repo or --repo-list\nusage: {USAGE}"
        )));
    }

    let mut seen = BTreeSet::new();
    let mut parsed = Vec::new();
    for repo in raw {
        let path = parse_repo(&repo)?;
        let key = (path.owner.clone(), path.name.clone());
        if seen.insert(key) {
            parsed.push(path);
        }
    }
    Ok(parsed)
}

fn parse_repo(repo: &str) -> Result<RepositoryPath, ArgsError> {
    let parts: Vec<&str> = repo.split('/').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        return Err(ArgsError::new(format!(
            "--repo must be owner/name with non-empty parts, got '{repo}'"
        )));
    }
    Ok(RepositoryPath::new(parts[0], parts[1]))
}

fn parse_role_decision_args_json(raw: Option<String>) -> Result<Vec<String>, ArgsError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    serde_json::from_str::<Vec<String>>(&raw).map_err(|error| {
        ArgsError::new(format!(
            "{ROLE_DECISION_ARGS_ENV} must be a JSON array of strings: {error}"
        ))
    })
}

fn parse_env_allowlist(raw: Option<String>) -> Vec<String> {
    raw.map(|raw| {
        raw.split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .collect()
    })
    .unwrap_or_default()
}

fn parse_role_decision_timeout_secs(raw: &str) -> Result<u64, ArgsError> {
    let value = raw.parse::<u64>().map_err(|_| {
        ArgsError::new(format!(
            "--role-decision-timeout-secs must be an integer, got '{raw}'"
        ))
    })?;
    if value == 0 {
        return Err(ArgsError::new(
            "--role-decision-timeout-secs must be positive",
        ));
    }
    Ok(value)
}

fn parse_audit_interval(raw: Option<String>) -> Result<Option<Duration>, ArgsError> {
    let value = match raw {
        Some(raw) => parse_i64_allow_zero(&raw, "--audit-ms")?,
        None => DEFAULT_AUDIT_INTERVAL_MS,
    };
    Ok((value > 0).then(|| Duration::milliseconds(value)))
}

fn parse_idle_poll_max_interval(
    raw: Option<String>,
    poll_interval: Duration,
) -> Result<Duration, ArgsError> {
    let configured = match raw {
        Some(raw) => Duration::milliseconds(parse_i64(&raw, "--idle-poll-max-ms")?),
        None => Duration::milliseconds(DEFAULT_IDLE_POLL_MAX_MS),
    };
    Ok(if configured < poll_interval {
        poll_interval
    } else {
        configured
    })
}

fn parse_i64(raw: &str, flag: &str) -> Result<i64, ArgsError> {
    let value = parse_i64_allow_zero(raw, flag)?;
    if value <= 0 {
        return Err(ArgsError::new(format!(
            "{flag} must be positive, got {value}"
        )));
    }
    Ok(value)
}

fn parse_i64_allow_zero(raw: &str, flag: &str) -> Result<i64, ArgsError> {
    let value = raw
        .parse::<i64>()
        .map_err(|_| ArgsError::new(format!("{flag} must be an integer, got '{raw}'")))?;
    if value < 0 {
        return Err(ArgsError::new(format!(
            "{flag} must be non-negative, got {value}"
        )));
    }
    Ok(value)
}

fn parse_u64(raw: &str, flag: &str) -> Result<u64, ArgsError> {
    let value = raw
        .parse::<u64>()
        .map_err(|_| ArgsError::new(format!("{flag} must be an integer, got '{raw}'")))?;
    if value == 0 {
        return Err(ArgsError::new(format!("{flag} must be positive, got 0")));
    }
    Ok(value)
}

fn require_env<E>(env: &E, key: &str) -> Result<String, ArgsError>
where
    E: Fn(&str) -> Option<String>,
{
    env(key)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ArgsError::new(format!("missing required environment variable {key}")))
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.and_then(|value| (!value.trim().is_empty()).then_some(value))
}

fn non_empty_env<E>(env: &E, key: &str) -> Option<String>
where
    E: Fn(&str) -> Option<String>,
{
    env(key).and_then(|value| (!value.trim().is_empty()).then_some(value))
}

#[cfg(test)]
#[path = "worker_args_tests.rs"]
mod tests;
