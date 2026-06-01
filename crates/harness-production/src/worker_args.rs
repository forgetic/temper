//! Argument parsing for `harness-worker`.

use chrono::Duration;
use harness_forge::RepositoryPath;
use std::collections::BTreeSet;
use std::fmt;
use std::path::PathBuf;

pub const FORGEJO_TOKEN_ENV: &str = "HARNESS_FORGEJO_TOKEN";
pub const FORGEJO_USERNAME_ENV: &str = "HARNESS_FORGEJO_USERNAME";
pub const FORGEJO_PASSWORD_ENV: &str = "HARNESS_FORGEJO_PASSWORD";
pub const AGENTS_AUTH_ENV: &str = "HARNESS_AGENTS_AUTH";

pub const USAGE: &str = concat!(
    "harness-worker --backend forgejo --base-url <url> (--repo <owner/name> [--repo <owner/name> ...] | --repo-list <path>) ",
    "--kind <role|mechanical> [--role <id> --user <handle>] ",
    "[--auth <deepseek|chatgpt-oauth|anthropic-oauth>] ",
    "[--codex-model <id>] [--auth-file <path>] ",
    "[--poll-ms <n>] [--stop-file <path>] [--run-secs <max>] ",
    "[--wake-socket <path>] [--wake-secret-file <path>]\n",
    "  forgejo token comes from HARNESS_FORGEJO_TOKEN; optional web UI credentials ",
    "come from HARNESS_FORGEJO_USERNAME/HARNESS_FORGEJO_PASSWORD"
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthKind {
    ChatGptOAuth,
    DeepSeek,
    AnthropicOAuth,
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

#[derive(Clone, Debug, Eq, PartialEq)]
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
    pub stop_file: Option<PathBuf>,
    pub run_secs: Option<u64>,
    pub auth: AuthKind,
    pub codex_model: Option<String>,
    pub auth_file: Option<PathBuf>,
    pub wake_socket: Option<PathBuf>,
    pub wake_secret_file: Option<PathBuf>,
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
    stop_file: Option<String>,
    run_secs: Option<String>,
    auth: Option<String>,
    codex_model: Option<String>,
    auth_file: Option<String>,
    wake_socket: Option<String>,
    wake_secret_file: Option<String>,
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
                "--stop-file" => raw.stop_file = Some(value_for(&flag, &mut iter)?),
                "--run-secs" => raw.run_secs = Some(value_for(&flag, &mut iter)?),
                "--auth" => raw.auth = Some(value_for(&flag, &mut iter)?),
                "--codex-model" => raw.codex_model = Some(value_for(&flag, &mut iter)?),
                "--auth-file" => raw.auth_file = Some(value_for(&flag, &mut iter)?),
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
        Ok(WorkerArgs {
            kind,
            forgejo,
            owner,
            name,
            repositories,
            poll_interval: Duration::milliseconds(match self.poll_ms {
                Some(raw) => parse_i64(&raw, "--poll-ms")?,
                None => 1_000,
            }),
            stop_file: self.stop_file.map(PathBuf::from),
            run_secs: self
                .run_secs
                .map(|raw| parse_u64(&raw, "--run-secs"))
                .transpose()?,
            auth: parse_auth(self.auth.as_deref(), env)?,
            codex_model: non_empty(self.codex_model),
            auth_file: non_empty(self.auth_file).map(PathBuf::from),
            wake_socket: non_empty(self.wake_socket).map(PathBuf::from),
            wake_secret_file: non_empty(self.wake_secret_file).map(PathBuf::from),
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

fn parse_auth<E>(raw: Option<&str>, env: &E) -> Result<AuthKind, ArgsError>
where
    E: Fn(&str) -> Option<String>,
{
    let selected = raw
        .map(str::to_string)
        .or_else(|| non_empty_env(env, AGENTS_AUTH_ENV))
        .unwrap_or_else(|| "chatgpt-oauth".to_string());
    match selected.as_str() {
        "chatgpt-oauth" => Ok(AuthKind::ChatGptOAuth),
        "deepseek" => Ok(AuthKind::DeepSeek),
        "anthropic-oauth" => Ok(AuthKind::AnthropicOAuth),
        other => Err(ArgsError::new(format!(
            "unknown --auth '{other}'; expected deepseek|chatgpt-oauth|anthropic-oauth"
        ))),
    }
}

fn parse_i64(raw: &str, flag: &str) -> Result<i64, ArgsError> {
    let value = raw
        .parse::<i64>()
        .map_err(|_| ArgsError::new(format!("{flag} must be an integer, got '{raw}'")))?;
    if value <= 0 {
        return Err(ArgsError::new(format!(
            "{flag} must be positive, got {value}"
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
mod tests {
    use super::*;

    fn env(key: &str) -> Option<String> {
        match key {
            FORGEJO_TOKEN_ENV => Some("secret-token".into()),
            _ => None,
        }
    }

    #[test]
    fn parses_role_worker_and_redacts_token_in_debug() {
        let outcome = parse_with_env(
            [
                "--backend",
                "forgejo",
                "--base-url",
                "http://127.0.0.1:3000",
                "--repo",
                "acme/service",
                "--kind",
                "role",
                "--role",
                "engineer",
                "--user",
                "engineer",
            ]
            .into_iter()
            .map(String::from),
            env,
        )
        .expect("parses");
        let ParseOutcome::Run(args) = outcome else {
            panic!("expected run")
        };
        assert_eq!(args.owner, "acme");
        assert_eq!(args.name, "service");
        assert_eq!(
            args.repositories,
            vec![RepositoryPath::new("acme", "service")]
        );
        assert!(format!("{:?}", args.forgejo).contains("<redacted>"));
        assert!(!format!("{:?}", args.forgejo).contains("secret-token"));
    }

    #[test]
    fn parses_optional_wake_socket() {
        let outcome = parse_with_env(
            [
                "--backend",
                "forgejo",
                "--base-url",
                "http://127.0.0.1:3000",
                "--repo",
                "acme/service",
                "--kind",
                "mechanical",
                "--wake-socket",
                "run/wake/mechanical.sock",
                "--wake-secret-file",
                "secrets/wake",
            ]
            .into_iter()
            .map(String::from),
            env,
        )
        .expect("parses");
        let ParseOutcome::Run(args) = outcome else {
            panic!("expected run")
        };
        assert_eq!(
            args.wake_socket,
            Some(PathBuf::from("run/wake/mechanical.sock"))
        );
        assert_eq!(args.wake_secret_file, Some(PathBuf::from("secrets/wake")));
    }

    #[test]
    fn parses_multiple_repos_and_deduplicates() {
        let outcome = parse_with_env(
            [
                "--backend",
                "forgejo",
                "--base-url",
                "http://127.0.0.1:3000",
                "--repo",
                "acme/service",
                "--repo",
                "acme/other",
                "--repo",
                "acme/service",
                "--kind",
                "mechanical",
            ]
            .into_iter()
            .map(String::from),
            env,
        )
        .expect("parses");
        let ParseOutcome::Run(args) = outcome else {
            panic!("expected run")
        };
        assert_eq!(
            args.repositories,
            vec![
                RepositoryPath::new("acme", "service"),
                RepositoryPath::new("acme", "other")
            ]
        );
    }

    #[test]
    fn parses_repo_list_file() {
        let path = std::env::temp_dir().join(format!(
            "harness-production-repos-{}-{}.txt",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        std::fs::write(
            &path,
            "# scan shard\nacme/service\nacme/other # inline comment\n",
        )
        .expect("repo-list writes");
        let outcome = parse_with_env(
            vec![
                "--backend".to_string(),
                "forgejo".to_string(),
                "--base-url".to_string(),
                "http://127.0.0.1:3000".to_string(),
                "--repo-list".to_string(),
                path.display().to_string(),
                "--kind".to_string(),
                "mechanical".to_string(),
            ],
            env,
        )
        .expect("parses");
        let ParseOutcome::Run(args) = outcome else {
            panic!("expected run")
        };
        assert_eq!(args.repositories.len(), 2);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rejects_malformed_repo_names() {
        let error = parse_with_env(
            [
                "--backend",
                "forgejo",
                "--base-url",
                "http://127.0.0.1:3000",
                "--repo",
                "acme/service/extra",
                "--kind",
                "mechanical",
            ]
            .into_iter()
            .map(String::from),
            env,
        )
        .unwrap_err();
        assert!(error.to_string().contains("owner/name"));
    }

    #[test]
    fn rejects_testing_only_backend() {
        let error = parse_with_env(
            ["--backend", "filesystem"].into_iter().map(String::from),
            env,
        )
        .unwrap_err();
        assert!(error.to_string().contains("expected forgejo"));
    }

    #[test]
    fn help_short_circuits_without_env() {
        assert_eq!(
            parse_with_env(["--help".to_string()], |_| None).unwrap(),
            ParseOutcome::Help
        );
    }
}
