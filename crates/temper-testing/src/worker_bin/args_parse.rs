//! Argument parsing for the `temper-testing-worker` binary.
//!
//! This is the hand-rolled, dependency-light parser split out of
//! [`super::args`] (which holds the parsed *types*): it walks `--flag value`
//! pairs into a [`RawArgs`] bag, then cross-validates them into a
//! [`WorkerArgs`]. Keep it dependency-light; if the surface grows past a handful
//! of flags, reconsider a small lockfile crate rather than hand-rolling more.

use super::args::{
    AgentsKind, ArchitectKind, ArgsError, Backend, BackendKind, CiPolicyKind, CiSentinelKind,
    ClockKind, ForgejoArgs, ReviewerKind, RoleBehavior, WorkerArgs, WorkerKind,
    FORGEJO_PASSWORD_ENV, FORGEJO_TOKEN_ENV, FORGEJO_USERNAME_ENV,
};
use chrono::Duration;
use std::collections::BTreeSet;
use std::path::PathBuf;
use temper_forge::RepositoryPath;

/// Outcome of parsing the raw argument vector.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseOutcome {
    /// A fully validated worker invocation.
    Run(Box<WorkerArgs>),
    /// `--help` was requested; the caller should print usage and exit zero.
    Help,
}

/// One-line usage string for `--help` and error context.
pub const USAGE: &str = concat!(
    "temper-testing-worker --kind <provision|role|mechanical|ci> --root <path> ",
    "--repo <owner/name> [--repo <owner/name> ...] [--backend <filesystem|forgejo>] [--base-url <url>] ",
    "[--role <id> --user <handle>] ",
    "[--architect <default|closing>] [--reviewer <default|request-changes-then-approve>] ",
    "[--ci <pass|fail-then-pass|fixed-fail>] [--ci-sentinel <present|deferred>] ",
    "[--agents <fake>] ",
    "[--poll-ms <n>] [--stop-file <path>] [--run-secs <max>] [--clock <deterministic|wall>] ",
    "[--wake-socket <path>] [--wake-secret-file <path>]\n",
    "  forgejo secrets come from the environment, never argv: ",
    "TEMPER_FORGEJO_TOKEN (required), TEMPER_FORGEJO_USERNAME/TEMPER_FORGEJO_PASSWORD ",
    "(optional, for the CI-reading role's web-UI login)",
);

/// Parses the process argument vector (excluding the program name).
///
/// Forgejo credentials are read from the process environment (see
/// [`FORGEJO_TOKEN_ENV`] and friends); use [`parse_with_env`] to inject a lookup
/// in tests.
pub fn parse<I>(args: I) -> Result<ParseOutcome, ArgsError>
where
    I: IntoIterator<Item = String>,
{
    parse_with_env(args, |key| std::env::var(key).ok())
}

/// Parses arguments, reading any required secrets through `env`.
///
/// `env` maps an environment variable name to its value (or `None`). The
/// production [`parse`] passes [`std::env::var`]; tests pass a fixed map so the
/// suite never touches the real environment.
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

/// Raw, loosely typed flag values before cross-field validation.
struct RawArgs {
    help: bool,
    kind: Option<String>,
    backend: Option<String>,
    base_url: Option<String>,
    root: Option<String>,
    repos: Vec<String>,
    role: Option<String>,
    user: Option<String>,
    architect: Option<String>,
    reviewer: Option<String>,
    ci: Option<String>,
    ci_sentinel: Option<String>,
    poll_ms: Option<String>,
    stop_file: Option<String>,
    run_secs: Option<String>,
    clock: Option<String>,
    agents: Option<String>,
    wake_socket: Option<String>,
    wake_secret_file: Option<String>,
}

impl RawArgs {
    fn collect<I>(args: I) -> Result<Self, ArgsError>
    where
        I: IntoIterator<Item = String>,
    {
        let mut raw = RawArgs {
            help: false,
            kind: None,
            backend: None,
            base_url: None,
            root: None,
            repos: Vec::new(),
            role: None,
            user: None,
            architect: None,
            reviewer: None,
            ci: None,
            ci_sentinel: None,
            poll_ms: None,
            stop_file: None,
            run_secs: None,
            clock: None,
            agents: None,
            wake_socket: None,
            wake_secret_file: None,
        };
        let mut iter = args.into_iter();
        while let Some(flag) = iter.next() {
            match flag.as_str() {
                "--help" | "-h" => raw.help = true,
                "--kind" => raw.kind = Some(value_for(&flag, &mut iter)?),
                "--backend" => raw.backend = Some(value_for(&flag, &mut iter)?),
                "--base-url" => raw.base_url = Some(value_for(&flag, &mut iter)?),
                "--root" => raw.root = Some(value_for(&flag, &mut iter)?),
                "--repo" => raw.repos.push(value_for(&flag, &mut iter)?),
                "--role" => raw.role = Some(value_for(&flag, &mut iter)?),
                "--user" => raw.user = Some(value_for(&flag, &mut iter)?),
                "--architect" => raw.architect = Some(value_for(&flag, &mut iter)?),
                "--reviewer" => raw.reviewer = Some(value_for(&flag, &mut iter)?),
                "--ci" => raw.ci = Some(value_for(&flag, &mut iter)?),
                "--ci-sentinel" => raw.ci_sentinel = Some(value_for(&flag, &mut iter)?),
                "--poll-ms" => raw.poll_ms = Some(value_for(&flag, &mut iter)?),
                "--stop-file" => raw.stop_file = Some(value_for(&flag, &mut iter)?),
                "--run-secs" => raw.run_secs = Some(value_for(&flag, &mut iter)?),
                "--clock" => raw.clock = Some(value_for(&flag, &mut iter)?),
                "--agents" => raw.agents = Some(value_for(&flag, &mut iter)?),
                "--wake-socket" => raw.wake_socket = Some(value_for(&flag, &mut iter)?),
                "--wake-secret-file" => raw.wake_secret_file = Some(value_for(&flag, &mut iter)?),
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
        let kind = self.parse_kind()?;
        let backend_kind = parse_backend(self.backend.as_deref())?;
        let root = PathBuf::from(require(self.root, "--root")?);
        let repositories = parse_repositories(self.repos)?;
        let first = repositories
            .first()
            .expect("parse_repositories returns at least one repository");
        let owner = first.owner.clone();
        let name = first.name.clone();
        let poll_interval = match self.poll_ms {
            Some(raw) => Duration::milliseconds(parse_i64(&raw, "--poll-ms")?),
            None => Duration::milliseconds(50),
        };
        let stop_file = self.stop_file.map(PathBuf::from);
        let run_secs = self
            .run_secs
            .map(|raw| parse_u64(&raw, "--run-secs"))
            .transpose()?;
        let clock = parse_clock(self.clock.as_deref())?;
        let agents = parse_agents(self.agents.as_deref())?;

        let backend = match backend_kind {
            BackendKind::Filesystem => {
                // `--base-url` is meaningless without a remote backend; reject it
                // rather than silently ignore, so a typo'd `--backend` surfaces.
                if self.base_url.is_some() {
                    return Err(ArgsError::new(
                        "--base-url is only valid with --backend forgejo".to_string(),
                    ));
                }
                Backend::Filesystem
            }
            BackendKind::Forgejo => {
                // CI on Forgejo is produced by the real `forgejo-runner` and read
                // via the Phase 3b web-UI path; there is no fake CI producer on
                // this backend (findings-phase-0b).
                if matches!(kind, WorkerKind::Ci { .. }) {
                    return Err(ArgsError::new(
                        "--kind ci is not supported with --backend forgejo: CI is produced by \
                         the real forgejo-runner, not a fake worker"
                            .to_string(),
                    ));
                }
                // The `ManualClock` epoch seam exists only for the filesystem
                // logical clock; a real server writes wall-clock timestamps.
                if clock != ClockKind::Wall {
                    return Err(ArgsError::new(
                        "--backend forgejo requires --clock wall (the deterministic ManualClock \
                         epoch seam is filesystem-only)"
                            .to_string(),
                    ));
                }
                let base_url =
                    require(self.base_url, "--base-url (required for --backend forgejo)")?;
                let token = require_env(env, FORGEJO_TOKEN_ENV)?;
                let username = non_empty_env(env, FORGEJO_USERNAME_ENV);
                let password = non_empty_env(env, FORGEJO_PASSWORD_ENV);
                Backend::Forgejo(ForgejoArgs {
                    base_url,
                    token,
                    username,
                    password,
                })
            }
        };

        Ok(WorkerArgs {
            kind,
            backend,
            root,
            owner,
            name,
            repositories,
            poll_interval,
            stop_file,
            run_secs,
            clock,
            agents,
            wake_socket: non_empty(self.wake_socket).map(PathBuf::from),
            wake_secret_file: non_empty(self.wake_secret_file).map(PathBuf::from),
        })
    }

    fn parse_kind(&self) -> Result<WorkerKind, ArgsError> {
        let kind = self
            .kind
            .as_deref()
            .ok_or_else(|| ArgsError::new(format!("missing required --kind\nusage: {USAGE}")))?;
        match kind {
            "provision" => Ok(WorkerKind::Provision),
            "mechanical" => Ok(WorkerKind::Mechanical),
            "ci" => Ok(WorkerKind::Ci {
                policy: parse_ci(self.ci.as_deref())?,
            }),
            "role" => {
                let role = require_ref(self.role.as_deref(), "--role (required for --kind role)")?;
                let user = require_ref(self.user.as_deref(), "--user (required for --kind role)")?;
                let behavior = RoleBehavior {
                    architect: parse_architect(self.architect.as_deref())?,
                    reviewer: parse_reviewer(self.reviewer.as_deref())?,
                    ci_sentinel: parse_ci_sentinel(self.ci_sentinel.as_deref())?,
                };
                Ok(WorkerKind::Role {
                    role,
                    user,
                    behavior,
                })
            }
            other => Err(ArgsError::new(format!(
                "unknown --kind '{other}'; expected provision|role|mechanical|ci"
            ))),
        }
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

fn parse_repositories(repos: Vec<String>) -> Result<Vec<RepositoryPath>, ArgsError> {
    if repos.is_empty() {
        return Err(ArgsError::new(format!(
            "missing required --repo\nusage: {USAGE}"
        )));
    }
    let mut seen = BTreeSet::new();
    let mut parsed = Vec::new();
    for repo in repos {
        let path = parse_repo(&repo)?;
        let key = (path.owner.clone(), path.name.clone());
        if seen.insert(key) {
            parsed.push(path);
        }
    }
    Ok(parsed)
}

fn parse_repo(repo: &str) -> Result<RepositoryPath, ArgsError> {
    let (owner, name) = repo
        .split_once('/')
        .ok_or_else(|| ArgsError::new(format!("--repo must be owner/name, got '{repo}'")))?;
    if owner.is_empty() || name.is_empty() {
        return Err(ArgsError::new(format!(
            "--repo must be owner/name with non-empty parts, got '{repo}'"
        )));
    }
    Ok(RepositoryPath::new(owner, name))
}

fn parse_ci(ci: Option<&str>) -> Result<CiPolicyKind, ArgsError> {
    match ci {
        None | Some("pass") => Ok(CiPolicyKind::Pass),
        Some("fail-then-pass") => Ok(CiPolicyKind::FailThenPass),
        Some("fixed-fail") => Ok(CiPolicyKind::FixedFail),
        Some(other) => Err(ArgsError::new(format!(
            "unknown --ci '{other}'; expected pass|fail-then-pass|fixed-fail"
        ))),
    }
}

fn parse_architect(architect: Option<&str>) -> Result<ArchitectKind, ArgsError> {
    match architect {
        None | Some("default") => Ok(ArchitectKind::Default),
        Some("closing") => Ok(ArchitectKind::Closing),
        Some(other) => Err(ArgsError::new(format!(
            "unknown --architect '{other}'; expected default|closing"
        ))),
    }
}

fn parse_reviewer(reviewer: Option<&str>) -> Result<ReviewerKind, ArgsError> {
    match reviewer {
        None | Some("default") => Ok(ReviewerKind::Default),
        Some("request-changes-then-approve") => Ok(ReviewerKind::RequestChangesThenApprove),
        Some(other) => Err(ArgsError::new(format!(
            "unknown --reviewer '{other}'; expected default|request-changes-then-approve"
        ))),
    }
}

fn parse_ci_sentinel(ci_sentinel: Option<&str>) -> Result<CiSentinelKind, ArgsError> {
    match ci_sentinel {
        None | Some("present") => Ok(CiSentinelKind::Present),
        Some("deferred") => Ok(CiSentinelKind::Deferred),
        Some(other) => Err(ArgsError::new(format!(
            "unknown --ci-sentinel '{other}'; expected present|deferred"
        ))),
    }
}

fn parse_backend(backend: Option<&str>) -> Result<BackendKind, ArgsError> {
    match backend {
        None | Some("filesystem") => Ok(BackendKind::Filesystem),
        Some("forgejo") => Ok(BackendKind::Forgejo),
        Some(other) => Err(ArgsError::new(format!(
            "unknown --backend '{other}'; expected filesystem|forgejo"
        ))),
    }
}

/// Reads a required secret from the environment, redacting the value on failure.
///
/// The error names only the variable, never the (absent) value, matching the
/// crate-wide guarantee that secrets never appear in logs or errors.
fn require_env<E>(env: &E, key: &'static str) -> Result<String, ArgsError>
where
    E: Fn(&str) -> Option<String>,
{
    non_empty_env(env, key)
        .ok_or_else(|| ArgsError::new(format!("missing required environment variable {key}")))
}

/// Reads an optional, non-empty secret from the environment.
fn non_empty_env<E>(env: &E, key: &str) -> Option<String>
where
    E: Fn(&str) -> Option<String>,
{
    env(key).filter(|value| !value.trim().is_empty())
}

fn parse_clock(clock: Option<&str>) -> Result<ClockKind, ArgsError> {
    match clock {
        None | Some("deterministic") => Ok(ClockKind::Deterministic),
        Some("wall") => Ok(ClockKind::Wall),
        Some(other) => Err(ArgsError::new(format!(
            "unknown --clock '{other}'; expected deterministic|wall"
        ))),
    }
}

fn parse_agents(agents: Option<&str>) -> Result<AgentsKind, ArgsError> {
    match agents {
        None | Some("fake") => Ok(AgentsKind::Fake),
        Some("real") => Err(ArgsError::new(
            "--agents real moved out of Temper; run Smith's workflow-role process e2e instead",
        )),
        Some(other) => Err(ArgsError::new(format!(
            "unknown --agents '{other}'; expected fake"
        ))),
    }
}

/// Trims and drops an empty CLI value to `None`.
fn non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|raw| raw.trim().to_string())
        .filter(|raw| !raw.is_empty())
}

fn parse_i64(raw: &str, flag: &str) -> Result<i64, ArgsError> {
    raw.parse::<i64>()
        .map_err(|_| ArgsError::new(format!("{flag} must be an integer, got '{raw}'")))
}

fn parse_u64(raw: &str, flag: &str) -> Result<u64, ArgsError> {
    raw.parse::<u64>().map_err(|_| {
        ArgsError::new(format!(
            "{flag} must be a non-negative integer, got '{raw}'"
        ))
    })
}

#[cfg(test)]
#[path = "args_parse_tests.rs"]
mod tests;
