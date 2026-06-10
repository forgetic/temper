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
    "  Forgejo connection settings come from FORGEJO_URL + ",
    "FORGEJO_ACCESS_TOKEN; optional Forgejo environment variables are read by ",
    "ForgejoConfig::from_env"
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
        let mechanical_cadence = parse_optional_positive_secs(
            self.mechanical_cadence_secs.as_deref(),
            "--mechanical-cadence-secs",
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

fn parse_optional_positive_secs(raw: Option<&str>, flag: &str) -> Result<Option<Duration>, String> {
    raw.map(|raw| parse_secs(raw, flag).and_then(|secs| positive_duration(secs, flag)))
        .transpose()
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
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| (*arg).to_string()).collect()
    }

    fn run(args: &[&str]) -> DaemonRunConfig {
        match parse(strings(args)).expect("arguments parse") {
            ParseOutcome::Run(config) => config,
            ParseOutcome::Help => panic!("expected run config"),
        }
    }

    fn repo(owner: &str, name: &str) -> RepositoryPath {
        RepositoryPath::new(owner, name)
    }

    fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }

    fn role(id: &str) -> RoleId {
        RoleId::new(id)
    }

    #[test]
    fn role_tokens_resolve_configured_roles_only() {
        let tokens = role_tokens_from_env(
            vec!["engineer".to_string(), "reviewer".to_string()],
            env(&[
                ("TEMPER_FORGEJO_TOKEN_ENGINEER", "engineer-token"),
                ("TEMPER_FORGEJO_TOKEN_ARCHITECT", "architect-token"),
                ("UNRELATED", "ignored"),
            ]),
        );

        assert_eq!(
            tokens,
            BTreeMap::from([("engineer".to_string(), "engineer-token".to_string())])
        );
    }

    #[test]
    fn role_tokens_map_hyphenated_and_non_alphanumeric_roles_to_env_keys() {
        let tokens = role_tokens_from_env(
            vec![
                "ci-reviewer".to_string(),
                "qa.bot/2".to_string(),
                "agent_007".to_string(),
            ],
            env(&[
                ("TEMPER_FORGEJO_TOKEN_CI_REVIEWER", "ci-token"),
                ("TEMPER_FORGEJO_TOKEN_QA_BOT_2", "qa-token"),
                ("TEMPER_FORGEJO_TOKEN_AGENT_007", "agent-token"),
            ]),
        );

        assert_eq!(
            tokens,
            BTreeMap::from([
                ("agent_007".to_string(), "agent-token".to_string()),
                ("ci-reviewer".to_string(), "ci-token".to_string()),
                ("qa.bot/2".to_string(), "qa-token".to_string()),
            ])
        );
    }

    #[test]
    fn role_tokens_treat_trimmed_empty_values_as_absent() {
        let tokens = role_tokens_from_env(
            vec!["engineer".to_string(), "reviewer".to_string()],
            env(&[
                ("TEMPER_FORGEJO_TOKEN_ENGINEER", "  "),
                ("TEMPER_FORGEJO_TOKEN_REVIEWER", "reviewer-token"),
            ]),
        );

        assert_eq!(
            tokens,
            BTreeMap::from([("reviewer".to_string(), "reviewer-token".to_string())])
        );
    }

    #[test]
    fn role_tokens_deduplicate_duplicate_roles() {
        let tokens = role_tokens_from_env(
            vec![
                "engineer".to_string(),
                "engineer".to_string(),
                "reviewer".to_string(),
            ],
            env(&[
                ("TEMPER_FORGEJO_TOKEN_ENGINEER", "engineer-token"),
                ("TEMPER_FORGEJO_TOKEN_REVIEWER", "reviewer-token"),
            ]),
        );

        assert_eq!(tokens.len(), 2);
        assert_eq!(
            tokens.get("engineer").map(String::as_str),
            Some("engineer-token")
        );
        assert_eq!(
            tokens.get("reviewer").map(String::as_str),
            Some("reviewer-token")
        );
    }

    #[test]
    fn defaults_apply_when_only_repo_and_role_given() {
        let config = run(&["--repo", "acme/service", "--role", "engineer"]);

        assert_eq!(config.bind, "127.0.0.1:8080".parse().unwrap());
        assert_eq!(config.repos, vec![repo("acme", "service")]);
        assert_eq!(config.roles, vec![role("engineer")]);
        assert_eq!(config.poll_cadence, Duration::from_secs(30));
        assert_eq!(config.mechanical_cadence, None);
        assert_eq!(config.lease_ttl, Duration::from_secs(300));
        assert_eq!(config.daemon_id, "temper-daemon-1");
        assert_eq!(config.workflow_file, None);
        assert_eq!(config.webhook_secret_file, None);
    }

    #[test]
    fn repeated_repo_and_role_accumulate_and_dedup() {
        let config = run(&[
            "--repo",
            "a/b",
            "--repo",
            "a/b",
            "--repo",
            "c/d",
            "--role",
            "engineer",
            "--role",
            "architect",
            "--role",
            "engineer",
        ]);

        assert_eq!(config.repos, vec![repo("a", "b"), repo("c", "d")]);
        assert_eq!(config.roles, vec![role("engineer"), role("architect")]);
    }

    #[test]
    fn missing_repo_is_rejected() {
        let error = parse(strings(&["--role", "engineer"])).unwrap_err();

        assert!(error.contains("--repo"));
    }

    #[test]
    fn missing_role_is_rejected() {
        let error = parse(strings(&["--repo", "a/b"])).unwrap_err();

        assert!(error.contains("--role"));
    }

    #[test]
    fn malformed_repo_is_rejected() {
        for raw in ["nope", "a/", "/b", "a/b/c"] {
            let error = parse(strings(&["--repo", raw, "--role", "engineer"])).unwrap_err();
            assert!(error.contains("--repo"), "error for {raw:?}: {error}");
        }
    }

    #[test]
    fn bind_and_cadence_and_ttl_and_secret_and_workflow_parse() {
        let config = run(&[
            "--repo",
            "acme/service",
            "--role",
            "engineer",
            "--bind",
            "127.0.0.1:1",
            "--bind",
            "0.0.0.0:9000",
            "--poll-cadence-secs",
            "1",
            "--poll-cadence-secs",
            "60",
            "--mechanical-cadence-secs",
            "7",
            "--mechanical-cadence-secs",
            "120",
            "--lease-ttl-secs",
            "2",
            "--lease-ttl-secs",
            "900",
            "--webhook-secret-file",
            "old-secret.txt",
            "--webhook-secret-file",
            "secret.txt",
            "--workflow",
            "old-workflow.json",
            "--workflow",
            "workflow.json",
            "--daemon-id",
            "old-daemon",
            "--daemon-id",
            " daemon-a ",
        ]);

        assert_eq!(config.bind, "0.0.0.0:9000".parse().unwrap());
        assert_eq!(config.poll_cadence, Duration::from_secs(60));
        assert_eq!(config.mechanical_cadence, Some(Duration::from_secs(120)));
        assert_eq!(config.lease_ttl, Duration::from_secs(900));
        assert_eq!(
            config.webhook_secret_file,
            Some(PathBuf::from("secret.txt"))
        );
        assert_eq!(config.workflow_file, Some(PathBuf::from("workflow.json")));
        assert_eq!(config.daemon_id, "daemon-a");
    }

    #[test]
    fn invalid_bind_is_rejected() {
        let error = parse(strings(&[
            "--repo",
            "a/b",
            "--role",
            "engineer",
            "--bind",
            "not-an-address",
        ]))
        .unwrap_err();

        assert!(error.contains("--bind"));
    }

    #[test]
    fn invalid_cadence_is_rejected() {
        for raw in ["nope", "0"] {
            let error = parse(strings(&[
                "--repo",
                "a/b",
                "--role",
                "engineer",
                "--poll-cadence-secs",
                raw,
            ]))
            .unwrap_err();
            assert!(
                error.contains("--poll-cadence-secs"),
                "error for {raw:?}: {error}"
            );
        }
    }

    #[test]
    fn invalid_mechanical_cadence_is_rejected() {
        for raw in ["nope", "0"] {
            let error = parse(strings(&[
                "--repo",
                "a/b",
                "--role",
                "engineer",
                "--mechanical-cadence-secs",
                raw,
            ]))
            .unwrap_err();
            assert!(
                error.contains("--mechanical-cadence-secs"),
                "error for {raw:?}: {error}"
            );
        }
    }

    #[test]
    fn invalid_ttl_is_rejected() {
        for raw in ["nope", "0"] {
            let error = parse(strings(&[
                "--repo",
                "a/b",
                "--role",
                "engineer",
                "--lease-ttl-secs",
                raw,
            ]))
            .unwrap_err();
            assert!(
                error.contains("--lease-ttl-secs"),
                "error for {raw:?}: {error}"
            );
        }
    }

    #[test]
    fn unknown_flag_is_rejected() {
        let error = parse(strings(&[
            "--repo",
            "a/b",
            "--role",
            "engineer",
            "--mystery",
        ]))
        .unwrap_err();

        assert!(error.contains("--mystery"));
    }

    #[test]
    fn missing_flag_value_is_rejected_without_consuming_next_flag() {
        let error = parse(strings(&["--repo", "a/b", "--role", "--bind", "bad"])).unwrap_err();

        assert!(error.contains("--role"));
    }

    #[test]
    fn help_flag_returns_help() {
        for flag in ["--help", "-h"] {
            assert_eq!(parse(strings(&[flag])), Ok(ParseOutcome::Help));
        }
    }
}
