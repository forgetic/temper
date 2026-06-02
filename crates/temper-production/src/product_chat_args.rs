//! Argument parsing for `temper-product-manager-chat`.

use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;

use temper_forge::RepositoryPath;

pub const HUMAN_TOKEN_ENV: &str = "TEMPER_PRODUCT_CHAT_HUMAN_TOKEN";
pub const PRODUCT_MANAGER_TOKEN_ENV: &str = "TEMPER_PRODUCT_CHAT_PRODUCT_MANAGER_TOKEN";
pub const SERVICE_TOKEN_ENV: &str = "TEMPER_PRODUCT_CHAT_SERVICE_TOKEN";
pub const AGENTS_AUTH_ENV: &str = "TEMPER_AGENTS_AUTH";
pub const DEFAULT_SERVICE_BIND: &str = "127.0.0.1:39200";

pub const USAGE: &str = concat!(
    "temper-product-manager-chat repl --base-url <url> --repo <owner/name> ",
    "[--auth <deepseek|chatgpt-oauth|anthropic-oauth>] ",
    "[--codex-model <id>] [--auth-file <path>] [--transcript-issue <n>]\n",
    "temper-product-manager-chat serve --base-url <url> --repo <owner/name> ",
    "[--bind <addr:port>] [--allow-non-loopback] ",
    "[--auth <deepseek|chatgpt-oauth|anthropic-oauth>] ",
    "[--codex-model <id>] [--auth-file <path>]\n",
    "  Forgejo tokens come from TEMPER_PRODUCT_CHAT_HUMAN_TOKEN and ",
    "TEMPER_PRODUCT_CHAT_PRODUCT_MANAGER_TOKEN; optional API bearer comes from ",
    "TEMPER_PRODUCT_CHAT_SERVICE_TOKEN; no secrets on argv"
);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseOutcome {
    Repl(Box<ProductChatArgs>),
    Serve(Box<ProductChatServeArgs>),
    Help,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthKind {
    ChatGptOAuth,
    DeepSeek,
    AnthropicOAuth,
}

#[derive(Clone, Eq, PartialEq)]
pub struct ProductChatArgs {
    pub base_url: String,
    pub repo: RepositoryPath,
    pub human_token: String,
    pub product_manager_token: String,
    pub auth: AuthKind,
    pub codex_model: Option<String>,
    pub auth_file: Option<PathBuf>,
    pub transcript_issue: Option<u64>,
}

impl fmt::Debug for ProductChatArgs {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProductChatArgs")
            .field("base_url", &self.base_url)
            .field("repo", &self.repo)
            .field("human_token", &"<redacted>")
            .field("product_manager_token", &"<redacted>")
            .field("auth", &self.auth)
            .field("codex_model", &self.codex_model)
            .field("auth_file", &self.auth_file)
            .field("transcript_issue", &self.transcript_issue)
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct ProductChatServeArgs {
    pub bind: SocketAddr,
    pub allow_non_loopback: bool,
    pub base_url: String,
    pub repo: RepositoryPath,
    pub human_token: String,
    pub product_manager_token: String,
    pub service_token: Option<String>,
    pub auth: AuthKind,
    pub codex_model: Option<String>,
    pub auth_file: Option<PathBuf>,
}

impl fmt::Debug for ProductChatServeArgs {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProductChatServeArgs")
            .field("bind", &self.bind)
            .field("allow_non_loopback", &self.allow_non_loopback)
            .field("base_url", &self.base_url)
            .field("repo", &self.repo)
            .field("human_token", &"<redacted>")
            .field("product_manager_token", &"<redacted>")
            .field(
                "service_token",
                &self.service_token.as_ref().map(|_| "<redacted>"),
            )
            .field("auth", &self.auth)
            .field("codex_model", &self.codex_model)
            .field("auth_file", &self.auth_file)
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
    let mut iter = args.into_iter();
    let Some(command) = iter.next() else {
        return Err(ArgsError::new(format!(
            "missing required subcommand 'repl' or 'serve'\nusage: {USAGE}"
        )));
    };
    match command.as_str() {
        "--help" | "-h" | "help" => Ok(ParseOutcome::Help),
        "repl" => {
            let raw = RawReplArgs::collect(iter)?;
            if raw.help {
                Ok(ParseOutcome::Help)
            } else {
                raw.into_args(&env)
                    .map(|args| ParseOutcome::Repl(Box::new(args)))
            }
        }
        "serve" => {
            let raw = RawServeArgs::collect(iter)?;
            if raw.help {
                Ok(ParseOutcome::Help)
            } else {
                raw.into_args(&env)
                    .map(|args| ParseOutcome::Serve(Box::new(args)))
            }
        }
        other => Err(ArgsError::new(format!(
            "unknown subcommand '{other}'; expected repl or serve\nusage: {USAGE}"
        ))),
    }
}

#[derive(Default)]
struct RawReplArgs {
    help: bool,
    base_url: Option<String>,
    repo: Option<String>,
    auth: Option<String>,
    codex_model: Option<String>,
    auth_file: Option<String>,
    transcript_issue: Option<String>,
}

impl RawReplArgs {
    fn collect<I>(args: I) -> Result<Self, ArgsError>
    where
        I: IntoIterator<Item = String>,
    {
        let mut raw = RawReplArgs::default();
        let mut iter = args.into_iter();
        while let Some(flag) = iter.next() {
            match flag.as_str() {
                "--help" | "-h" => raw.help = true,
                "--base-url" => raw.base_url = Some(value_for(&flag, &mut iter)?),
                "--repo" => raw.repo = Some(value_for(&flag, &mut iter)?),
                "--auth" => raw.auth = Some(value_for(&flag, &mut iter)?),
                "--codex-model" => raw.codex_model = Some(value_for(&flag, &mut iter)?),
                "--auth-file" => raw.auth_file = Some(value_for(&flag, &mut iter)?),
                "--transcript-issue" => raw.transcript_issue = Some(value_for(&flag, &mut iter)?),
                other => {
                    return Err(ArgsError::new(format!(
                        "unrecognized argument '{other}'\nusage: {USAGE}"
                    )))
                }
            }
        }
        Ok(raw)
    }

    fn into_args<E>(self, env: &E) -> Result<ProductChatArgs, ArgsError>
    where
        E: Fn(&str) -> Option<String>,
    {
        Ok(ProductChatArgs {
            base_url: require(self.base_url, "--base-url")?,
            repo: parse_repo(&require(self.repo, "--repo")?)?,
            human_token: require_env(env, HUMAN_TOKEN_ENV)?,
            product_manager_token: require_env(env, PRODUCT_MANAGER_TOKEN_ENV)?,
            auth: parse_auth(self.auth.as_deref(), env)?,
            codex_model: non_empty(self.codex_model),
            auth_file: non_empty(self.auth_file).map(PathBuf::from),
            transcript_issue: self
                .transcript_issue
                .map(|raw| parse_issue_number(&raw, "--transcript-issue"))
                .transpose()?,
        })
    }
}

#[derive(Default)]
struct RawServeArgs {
    help: bool,
    bind: Option<String>,
    allow_non_loopback: bool,
    base_url: Option<String>,
    repo: Option<String>,
    auth: Option<String>,
    codex_model: Option<String>,
    auth_file: Option<String>,
}

impl RawServeArgs {
    fn collect<I>(args: I) -> Result<Self, ArgsError>
    where
        I: IntoIterator<Item = String>,
    {
        let mut raw = RawServeArgs::default();
        let mut iter = args.into_iter();
        while let Some(flag) = iter.next() {
            match flag.as_str() {
                "--help" | "-h" => raw.help = true,
                "--bind" => raw.bind = Some(value_for(&flag, &mut iter)?),
                "--allow-non-loopback" => raw.allow_non_loopback = true,
                "--base-url" => raw.base_url = Some(value_for(&flag, &mut iter)?),
                "--repo" => raw.repo = Some(value_for(&flag, &mut iter)?),
                "--auth" => raw.auth = Some(value_for(&flag, &mut iter)?),
                "--codex-model" => raw.codex_model = Some(value_for(&flag, &mut iter)?),
                "--auth-file" => raw.auth_file = Some(value_for(&flag, &mut iter)?),
                other => {
                    return Err(ArgsError::new(format!(
                        "unrecognized argument '{other}'\nusage: {USAGE}"
                    )))
                }
            }
        }
        Ok(raw)
    }

    fn into_args<E>(self, env: &E) -> Result<ProductChatServeArgs, ArgsError>
    where
        E: Fn(&str) -> Option<String>,
    {
        let bind = parse_bind(
            self.bind.as_deref().unwrap_or(DEFAULT_SERVICE_BIND),
            "--bind",
        )?;
        let service_token = non_empty_env(env, SERVICE_TOKEN_ENV);
        validate_bind(bind, self.allow_non_loopback, service_token.as_deref())?;
        Ok(ProductChatServeArgs {
            bind,
            allow_non_loopback: self.allow_non_loopback,
            base_url: require(self.base_url, "--base-url")?,
            repo: parse_repo(&require(self.repo, "--repo")?)?,
            human_token: require_env(env, HUMAN_TOKEN_ENV)?,
            product_manager_token: require_env(env, PRODUCT_MANAGER_TOKEN_ENV)?,
            service_token,
            auth: parse_auth(self.auth.as_deref(), env)?,
            codex_model: non_empty(self.codex_model),
            auth_file: non_empty(self.auth_file).map(PathBuf::from),
        })
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

fn parse_bind(raw: &str, flag: &str) -> Result<SocketAddr, ArgsError> {
    raw.parse()
        .map_err(|_| ArgsError::new(format!("{flag} must be addr:port, got '{raw}'")))
}

fn validate_bind(
    bind: SocketAddr,
    allow_non_loopback: bool,
    service_token: Option<&str>,
) -> Result<(), ArgsError> {
    if bind.ip().is_loopback() {
        return Ok(());
    }
    if !allow_non_loopback {
        return Err(ArgsError::new(
            "non-loopback --bind requires explicit --allow-non-loopback",
        ));
    }
    if service_token.is_none() {
        return Err(ArgsError::new(format!(
            "non-loopback --bind requires environment variable {SERVICE_TOKEN_ENV}"
        )));
    }
    Ok(())
}

fn parse_repo(repo: &str) -> Result<RepositoryPath, ArgsError> {
    let (owner, name) = repo
        .split_once('/')
        .ok_or_else(|| ArgsError::new(format!("--repo must be owner/name, got '{repo}'")))?;
    if owner.is_empty() || name.is_empty() || name.contains('/') {
        return Err(ArgsError::new(format!(
            "--repo must be owner/name with non-empty parts, got '{repo}'"
        )));
    }
    Ok(RepositoryPath::new(owner, name))
}

fn parse_issue_number(raw: &str, flag: &str) -> Result<u64, ArgsError> {
    let value = raw
        .parse::<u64>()
        .map_err(|_| ArgsError::new(format!("{flag} must be an integer, got '{raw}'")))?;
    if value == 0 {
        return Err(ArgsError::new(format!("{flag} must be positive, got 0")));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(key: &str) -> Option<String> {
        match key {
            HUMAN_TOKEN_ENV => Some("human-secret".into()),
            PRODUCT_MANAGER_TOKEN_ENV => Some("pm-secret".into()),
            _ => None,
        }
    }

    #[test]
    fn product_chat_args_parse_repl_and_redact_tokens_in_debug() {
        let outcome = parse_with_env(
            [
                "repl",
                "--base-url",
                "https://git.example.test",
                "--repo",
                "ai/temper",
                "--auth",
                "chatgpt-oauth",
                "--codex-model",
                "gpt-5.5",
                "--auth-file",
                "/tmp/auth.json",
                "--transcript-issue",
                "3",
            ]
            .into_iter()
            .map(String::from),
            env,
        )
        .expect("parses");
        let ParseOutcome::Repl(args) = outcome else {
            panic!("expected repl")
        };
        assert_eq!(args.repo, RepositoryPath::new("ai", "temper"));
        assert_eq!(args.auth, AuthKind::ChatGptOAuth);
        assert_eq!(args.transcript_issue, Some(3));
        let debug = format!("{args:?}");
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("human-secret"));
        assert!(!debug.contains("pm-secret"));
    }

    #[test]
    fn product_chat_args_default_auth_comes_from_env_then_chatgpt() {
        let outcome = parse_with_env(
            [
                "repl",
                "--base-url",
                "https://git.example.test",
                "--repo",
                "ai/temper",
            ]
            .into_iter()
            .map(String::from),
            |key| match key {
                HUMAN_TOKEN_ENV => Some("human-secret".into()),
                PRODUCT_MANAGER_TOKEN_ENV => Some("pm-secret".into()),
                AGENTS_AUTH_ENV => Some("anthropic-oauth".into()),
                _ => None,
            },
        )
        .expect("parses");
        let ParseOutcome::Repl(args) = outcome else {
            panic!("expected repl")
        };
        assert_eq!(args.auth, AuthKind::AnthropicOAuth);
    }

    #[test]
    fn product_chat_args_reject_missing_tokens() {
        let error = parse_with_env(
            [
                "repl",
                "--base-url",
                "https://git.example.test",
                "--repo",
                "ai/temper",
            ]
            .into_iter()
            .map(String::from),
            |_| None,
        )
        .unwrap_err();
        assert!(error.to_string().contains(HUMAN_TOKEN_ENV));
    }
}
