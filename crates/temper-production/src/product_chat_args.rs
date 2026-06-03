//! Argument parsing for the product-manager interactive-profile wrapper.

use std::fmt;
use std::net::SocketAddr;
use std::time::Duration;

use temper_forge::RepositoryPath;
use temper_interaction::ProcessResponderConfig;

pub const HUMAN_TOKEN_ENV: &str = "TEMPER_PRODUCT_CHAT_HUMAN_TOKEN";
pub const PRODUCT_MANAGER_TOKEN_ENV: &str = "TEMPER_PRODUCT_CHAT_PRODUCT_MANAGER_TOKEN";
pub const SERVICE_TOKEN_ENV: &str = "TEMPER_PRODUCT_CHAT_SERVICE_TOKEN";
pub const PROCESS_RESPONDER_COMMAND_ENV: &str = "TEMPER_PRODUCT_CHAT_RESPONDER_COMMAND";
pub const PROCESS_RESPONDER_ARGS_ENV: &str = "TEMPER_PRODUCT_CHAT_RESPONDER_ARGS_JSON";
pub const PROCESS_RESPONDER_CWD_ENV: &str = "TEMPER_PRODUCT_CHAT_RESPONDER_CWD";
pub const PROCESS_RESPONDER_ENV_ALLOWLIST_ENV: &str = "TEMPER_PRODUCT_CHAT_RESPONDER_ENV_ALLOWLIST";
pub const PROCESS_RESPONDER_TIMEOUT_ENV: &str = "TEMPER_PRODUCT_CHAT_RESPONDER_TIMEOUT_SECS";
pub const DEFAULT_SERVICE_BIND: &str = "127.0.0.1:39200";

pub const USAGE: &str = concat!(
    "temper-product-manager-chat repl --base-url <url> --repo <owner/name> ",
    "[--transcript-issue <n>] ",
    "--responder-command <path> [--responder-arg <arg>] ",
    "[--responder-env <name>] [--responder-cwd <path>] ",
    "[--responder-timeout-secs <n>]\n",
    "temper-product-manager-chat serve --base-url <url> --repo <owner/name> ",
    "[--bind <addr:port>] [--allow-non-loopback] ",
    "--responder-command <path> [--responder-arg <arg>] ",
    "[--responder-env <name>] [--responder-cwd <path>] ",
    "[--responder-timeout-secs <n>]\n",
    "  Forgejo tokens come from TEMPER_PRODUCT_CHAT_HUMAN_TOKEN and ",
    "TEMPER_PRODUCT_CHAT_PRODUCT_MANAGER_TOKEN; optional API bearer comes from ",
    "TEMPER_PRODUCT_CHAT_SERVICE_TOKEN; responder credentials belong to the ",
    "configured process, not Temper"
);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseOutcome {
    Repl(Box<ProductChatArgs>),
    Serve(Box<ProductChatServeArgs>),
    Help,
}

#[derive(Clone, Eq, PartialEq)]
pub struct ProductChatArgs {
    pub base_url: String,
    pub repo: RepositoryPath,
    pub human_token: String,
    pub product_manager_token: String,
    pub transcript_issue: Option<u64>,
    pub process_responder: Option<ProcessResponderConfig>,
}

impl fmt::Debug for ProductChatArgs {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProductChatArgs")
            .field("base_url", &self.base_url)
            .field("repo", &self.repo)
            .field("human_token", &"<redacted>")
            .field("product_manager_token", &"<redacted>")
            .field("transcript_issue", &self.transcript_issue)
            .field(
                "process_responder",
                &self.process_responder.as_ref().map(|_| "<configured>"),
            )
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
    pub process_responder: Option<ProcessResponderConfig>,
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
            .field(
                "process_responder",
                &self.process_responder.as_ref().map(|_| "<configured>"),
            )
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
    transcript_issue: Option<String>,
    responder: RawProcessResponderArgs,
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
                "--transcript-issue" => raw.transcript_issue = Some(value_for(&flag, &mut iter)?),
                "--responder-command" => raw.responder.command = Some(value_for(&flag, &mut iter)?),
                "--responder-arg" => raw.responder.args.push(value_for(&flag, &mut iter)?),
                "--responder-cwd" => raw.responder.cwd = Some(value_for(&flag, &mut iter)?),
                "--responder-env" => raw
                    .responder
                    .env_allowlist
                    .push(value_for(&flag, &mut iter)?),
                "--responder-timeout-secs" => {
                    raw.responder.timeout_secs = Some(value_for(&flag, &mut iter)?)
                }
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
            transcript_issue: self
                .transcript_issue
                .map(|raw| parse_issue_number(&raw, "--transcript-issue"))
                .transpose()?,
            process_responder: require_process_responder(self.responder.into_config(env)?)?,
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
    responder: RawProcessResponderArgs,
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
                "--responder-command" => raw.responder.command = Some(value_for(&flag, &mut iter)?),
                "--responder-arg" => raw.responder.args.push(value_for(&flag, &mut iter)?),
                "--responder-cwd" => raw.responder.cwd = Some(value_for(&flag, &mut iter)?),
                "--responder-env" => raw
                    .responder
                    .env_allowlist
                    .push(value_for(&flag, &mut iter)?),
                "--responder-timeout-secs" => {
                    raw.responder.timeout_secs = Some(value_for(&flag, &mut iter)?)
                }
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
            process_responder: require_process_responder(self.responder.into_config(env)?)?,
        })
    }
}

#[derive(Default)]
struct RawProcessResponderArgs {
    command: Option<String>,
    args: Vec<String>,
    cwd: Option<String>,
    env_allowlist: Vec<String>,
    timeout_secs: Option<String>,
}

impl RawProcessResponderArgs {
    fn into_config<E>(self, env: &E) -> Result<Option<ProcessResponderConfig>, ArgsError>
    where
        E: Fn(&str) -> Option<String>,
    {
        let Some(command) =
            non_empty(self.command).or_else(|| non_empty_env(env, PROCESS_RESPONDER_COMMAND_ENV))
        else {
            return Ok(None);
        };
        let args = if self.args.is_empty() {
            parse_responder_args_json(non_empty_env(env, PROCESS_RESPONDER_ARGS_ENV))?
        } else {
            self.args
        };
        let cwd = non_empty(self.cwd).or_else(|| non_empty_env(env, PROCESS_RESPONDER_CWD_ENV));
        let env_allowlist = if self.env_allowlist.is_empty() {
            parse_env_allowlist(non_empty_env(env, PROCESS_RESPONDER_ENV_ALLOWLIST_ENV))
        } else {
            self.env_allowlist
        };
        let timeout_secs = match self
            .timeout_secs
            .or_else(|| non_empty_env(env, PROCESS_RESPONDER_TIMEOUT_ENV))
        {
            Some(raw) => parse_timeout_secs(&raw)?,
            None => ProcessResponderConfig::DEFAULT_TIMEOUT.as_secs(),
        };
        let mut config = ProcessResponderConfig::new(command)
            .with_args(args)
            .with_env_allowlist(env_allowlist)
            .with_timeout(Duration::from_secs(timeout_secs));
        if let Some(cwd) = cwd {
            config = config.with_working_dir(cwd);
        }
        Ok(Some(config))
    }
}

fn require_process_responder(
    config: Option<ProcessResponderConfig>,
) -> Result<Option<ProcessResponderConfig>, ArgsError> {
    if config.is_some() {
        return Ok(config);
    }
    Err(ArgsError::new(
        "product-manager chat requires --responder-command or TEMPER_PRODUCT_CHAT_RESPONDER_COMMAND",
    ))
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

fn parse_responder_args_json(raw: Option<String>) -> Result<Vec<String>, ArgsError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    serde_json::from_str::<Vec<String>>(&raw).map_err(|error| {
        ArgsError::new(format!(
            "{PROCESS_RESPONDER_ARGS_ENV} must be a JSON array of strings: {error}"
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

fn parse_timeout_secs(raw: &str) -> Result<u64, ArgsError> {
    let value = raw.parse::<u64>().map_err(|_| {
        ArgsError::new(format!(
            "--responder-timeout-secs must be an integer, got '{raw}'"
        ))
    })?;
    if value == 0 {
        return Err(ArgsError::new("--responder-timeout-secs must be positive"));
    }
    Ok(value)
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
