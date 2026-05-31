//! Argument parsing for `harness-worker`.

use chrono::Duration;
use std::fmt;
use std::path::PathBuf;

pub const FORGEJO_TOKEN_ENV: &str = "HARNESS_FORGEJO_TOKEN";
pub const FORGEJO_USERNAME_ENV: &str = "HARNESS_FORGEJO_USERNAME";
pub const FORGEJO_PASSWORD_ENV: &str = "HARNESS_FORGEJO_PASSWORD";
pub const AGENTS_AUTH_ENV: &str = "HARNESS_AGENTS_AUTH";

pub const USAGE: &str = concat!(
    "harness-worker --backend forgejo --base-url <url> --repo <owner/name> ",
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
    pub owner: String,
    pub name: String,
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
    repo: Option<String>,
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
                "--repo" => raw.repo = Some(value_for(&flag, &mut iter)?),
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
        let (owner, name) = parse_repo(&require(self.repo, "--repo")?)?;
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

fn parse_repo(repo: &str) -> Result<(String, String), ArgsError> {
    let (owner, name) = repo
        .split_once('/')
        .ok_or_else(|| ArgsError::new(format!("--repo must be owner/name, got '{repo}'")))?;
    if owner.is_empty() || name.is_empty() {
        return Err(ArgsError::new(format!(
            "--repo must be owner/name with non-empty parts, got '{repo}'"
        )));
    }
    Ok((owner.to_string(), name.to_string()))
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
