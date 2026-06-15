// SPDX-License-Identifier: MPL-2.0

//! Pure deployment configuration parser for the root `temper-daemon` binary.

use std::{
    collections::{BTreeMap, BTreeSet},
    net::SocketAddr,
    path::PathBuf,
    time::Duration,
};

use temper_forge::RepositoryPath;
use temper_workflow::RoleId;

const DEFAULT_BIND: &str = "127.0.0.1:8080";
const DEFAULT_POLL_CADENCE_SECS: u64 = 30;
/// Mechanical backstop runs by default. Webhooks are the primary reaction path,
/// so this level-triggered safety net uses a conservative cadence. Pass
/// `--mechanical-cadence-secs 0` to disable the mechanical worker entirely.
const DEFAULT_MECHANICAL_CADENCE_SECS: u64 = 120;
const DEFAULT_LEASE_TTL_SECS: u64 = 300;
const DEFAULT_DAEMON_ID: &str = "temper-daemon-1";

/// Command-line usage for the deployable daemon binary.
pub const USAGE: &str = concat!(
    "temper-daemon [--bind <addr:port>] ",
    "--repo <owner/name> [--repo <owner/name> ...] ",
    "--role <role> [--role <role> ...] ",
    "[--workflow <path>] [--poll-cadence-secs <n>] ",
    "[--mechanical-cadence-secs <n>] [--lease-ttl-secs <n>] ",
    "[--webhook-secret-file <path>] [--daemon-id <id>]\n",
    "  Forgejo connection settings (URL, admin token, optional CI web-UI ",
    "credentials) come from the resolved temper config, translated by ",
    "temper-engine-service's forgejo_config adapter"
);

/// Fully parsed daemon runtime configuration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DaemonRunConfig {
    pub bind: SocketAddr,
    pub repos: Vec<RepositoryPath>,
    pub roles: Vec<RoleId>,
    pub workflow_file: Option<PathBuf>,
    pub poll_cadence: Duration,
    pub mechanical_cadence: Option<Duration>,
    pub lease_ttl: Duration,
    pub webhook_secret_file: Option<PathBuf>,
    pub daemon_id: String,
}

/// Result of parsing command-line arguments.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseOutcome {
    Help,
    Run(DaemonRunConfig),
}

/// Parses the deployable daemon command-line surface.
pub fn parse(args: impl IntoIterator<Item = String>) -> Result<ParseOutcome, String> {
    let raw = RawArgs::collect(args)?;
    if raw.help {
        return Ok(ParseOutcome::Help);
    }
    raw.into_config().map(ParseOutcome::Run)
}

/// Resolves per-role Forge API tokens from `TEMPER_FORGEJO_TOKEN_<ROLEKEY>`
/// environment variables.
///
/// Roles without a provisioned token are absent from the returned map so callers
/// can fall back to the default identity for them. `ROLEKEY` is the role id
/// uppercased with every non-`[A-Z0-9]` character replaced by `_`.
pub fn role_tokens_from_env(
    roles: impl IntoIterator<Item = String>,
    vars: impl IntoIterator<Item = (String, String)>,
) -> BTreeMap<String, String> {
    let wanted_roles = roles.into_iter().collect::<BTreeSet<_>>();
    let mut tokens = BTreeMap::new();

    for (key, token) in vars {
        let Some(role_key) = key.strip_prefix("TEMPER_FORGEJO_TOKEN_") else {
            continue;
        };
        if token.trim().is_empty() {
            continue;
        }

        for role in &wanted_roles {
            if role_key == env_role_key(role) {
                tokens.entry(role.clone()).or_insert_with(|| token.clone());
            }
        }
    }

    tokens
}

fn env_role_key(role: &str) -> String {
    role.chars()
        .map(|ch| {
            let ch = ch.to_ascii_uppercase();
            if ch.is_ascii_uppercase() || ch.is_ascii_digit() {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[derive(Default)]
struct RawArgs {
    help: bool,
    bind: Option<String>,
    repos: Vec<String>,
    roles: Vec<String>,
    workflow_file: Option<String>,
    poll_cadence_secs: Option<String>,
    mechanical_cadence_secs: Option<String>,
    lease_ttl_secs: Option<String>,
    webhook_secret_file: Option<String>,
    daemon_id: Option<String>,
}

impl RawArgs {
    fn collect(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut raw = Self::default();
        let mut iter = args.into_iter().peekable();
        while let Some(flag) = iter.next() {
            match flag.as_str() {
                "--help" | "-h" => raw.help = true,
                "--bind" => raw.bind = Some(value_for(&flag, &mut iter)?),
                "--repo" => raw.repos.push(value_for(&flag, &mut iter)?),
                "--role" => raw.roles.push(value_for(&flag, &mut iter)?),
                "--workflow" => raw.workflow_file = Some(value_for(&flag, &mut iter)?),
                "--poll-cadence-secs" => raw.poll_cadence_secs = Some(value_for(&flag, &mut iter)?),
                "--mechanical-cadence-secs" => {
                    raw.mechanical_cadence_secs = Some(value_for(&flag, &mut iter)?)
                }
                "--lease-ttl-secs" => raw.lease_ttl_secs = Some(value_for(&flag, &mut iter)?),
                "--webhook-secret-file" => {
                    raw.webhook_secret_file = Some(value_for(&flag, &mut iter)?)
                }
                "--daemon-id" => raw.daemon_id = Some(value_for(&flag, &mut iter)?),
                other => return Err(format!("unknown flag {other}")),
            }
        }
        Ok(raw)
    }

    fn into_config(self) -> Result<DaemonRunConfig, String> {
        let bind = parse_bind(self.bind.as_deref().unwrap_or(DEFAULT_BIND))?;
        let repos = parse_repos(self.repos)?;
        let roles = parse_roles(self.roles)?;
        let poll_cadence = parse_positive_secs(
            self.poll_cadence_secs.as_deref(),
            "--poll-cadence-secs",
            DEFAULT_POLL_CADENCE_SECS,
        )?;
        let mechanical_cadence = parse_disableable_secs(
            self.mechanical_cadence_secs.as_deref(),
            "--mechanical-cadence-secs",
            DEFAULT_MECHANICAL_CADENCE_SECS,
        )?;
        let lease_ttl = parse_positive_secs(
            self.lease_ttl_secs.as_deref(),
            "--lease-ttl-secs",
            DEFAULT_LEASE_TTL_SECS,
        )?;
        let daemon_id = parse_daemon_id(self.daemon_id)?;

        Ok(DaemonRunConfig {
            bind,
            repos,
            roles,
            workflow_file: self.workflow_file.map(PathBuf::from),
            poll_cadence,
            mechanical_cadence,
            lease_ttl,
            webhook_secret_file: self.webhook_secret_file.map(PathBuf::from),
            daemon_id,
        })
    }
}

fn value_for<I>(flag: &str, iter: &mut std::iter::Peekable<I>) -> Result<String, String>
where
    I: Iterator<Item = String>,
{
    match iter.peek() {
        Some(value) if looks_like_flag(value) => Err(format!("missing value for {flag}")),
        Some(_) => Ok(iter.next().expect("peeked value is present")),
        None => Err(format!("missing value for {flag}")),
    }
}

fn looks_like_flag(value: &str) -> bool {
    value.starts_with("--") || value == "-h"
}

fn parse_bind(raw: &str) -> Result<SocketAddr, String> {
    raw.parse::<SocketAddr>()
        .map_err(|error| format!("invalid --bind {raw:?}: {error}"))
}

fn parse_repos(raw: Vec<String>) -> Result<Vec<RepositoryPath>, String> {
    if raw.is_empty() {
        return Err("missing required --repo".to_string());
    }

    let mut repos = Vec::new();
    for repo in raw {
        let path = parse_repo(&repo)?;
        if !repos.iter().any(|existing: &RepositoryPath| {
            existing.owner == path.owner && existing.name == path.name
        }) {
            repos.push(path);
        }
    }
    Ok(repos)
}

fn parse_repo(repo: &str) -> Result<RepositoryPath, String> {
    let parts: Vec<&str> = repo.split('/').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        return Err(format!(
            "--repo must be owner/name with non-empty parts, got '{repo}'"
        ));
    }
    Ok(RepositoryPath::new(parts[0], parts[1]))
}

fn parse_roles(raw: Vec<String>) -> Result<Vec<RoleId>, String> {
    if raw.is_empty() {
        return Err("missing required --role".to_string());
    }

    let mut roles = Vec::new();
    for role in raw {
        let role = RoleId::new(role);
        if !roles.iter().any(|existing| existing == &role) {
            roles.push(role);
        }
    }
    Ok(roles)
}

fn parse_positive_secs(
    raw: Option<&str>,
    flag: &str,
    default_secs: u64,
) -> Result<Duration, String> {
    let secs = match raw {
        Some(raw) => parse_secs(raw, flag)?,
        None => default_secs,
    };
    positive_duration(secs, flag)
}

/// Parses a cadence that is on by default and can be explicitly disabled with
/// `0`. An unset value falls back to `default_secs`; an explicit `0` yields
/// `None` (disabled); any other value must be a positive number of seconds.
fn parse_disableable_secs(
    raw: Option<&str>,
    flag: &str,
    default_secs: u64,
) -> Result<Option<Duration>, String> {
    let secs = match raw {
        Some(raw) => parse_secs(raw, flag)?,
        None => default_secs,
    };
    match secs {
        0 => Ok(None),
        secs => Ok(Some(Duration::from_secs(secs))),
    }
}

fn parse_secs(raw: &str, flag: &str) -> Result<u64, String> {
    raw.parse::<u64>()
        .map_err(|error| format!("{flag} must be a positive integer: {error}"))
}

fn positive_duration(secs: u64, flag: &str) -> Result<Duration, String> {
    if secs == 0 {
        return Err(format!("{flag} must be positive"));
    }
    Ok(Duration::from_secs(secs))
}

fn parse_daemon_id(raw: Option<String>) -> Result<String, String> {
    let daemon_id = raw.unwrap_or_else(|| DEFAULT_DAEMON_ID.to_string());
    let daemon_id = daemon_id.trim().to_string();
    if daemon_id.is_empty() {
        return Err("--daemon-id must be non-empty".to_string());
    }
    Ok(daemon_id)
}

#[cfg(test)]
mod config_tests;
