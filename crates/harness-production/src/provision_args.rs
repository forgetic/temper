//! Argument parsing and top-level runner for `harness-provision-forgejo`.

use std::fmt;
use std::path::PathBuf;

use crate::provision::{self, ProvisionError};

pub const ADMIN_TOKEN_ENV: &str = "HARNESS_FORGEJO_ADMIN_TOKEN";

pub const USAGE: &str = concat!(
    "harness-provision-forgejo --base-url <url> --owner <org> --name <repo> --out <path> ",
    "[--webhook-url <url> --webhook-secret-file <path>]\n",
    "  the admin token comes from HARNESS_FORGEJO_ADMIN_TOKEN (required), never argv"
);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseOutcome {
    Run(ProvisionArgs),
    Help,
}

#[derive(Clone, Eq, PartialEq)]
pub struct ProvisionArgs {
    pub base_url: String,
    pub owner: String,
    pub name: String,
    pub out: PathBuf,
    pub admin_token: String,
    pub webhook_url: Option<String>,
    pub webhook_secret_file: Option<PathBuf>,
}

impl fmt::Debug for ProvisionArgs {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProvisionArgs")
            .field("base_url", &self.base_url)
            .field("owner", &self.owner)
            .field("name", &self.name)
            .field("out", &self.out)
            .field("admin_token", &"<redacted>")
            .field("webhook_url", &self.webhook_url)
            .field("webhook_secret_file", &self.webhook_secret_file)
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

#[derive(Debug)]
pub enum RunError {
    Runtime(String),
    Provision(ProvisionError),
}

impl fmt::Display for RunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::Runtime(why) => write!(formatter, "failed to start async runtime: {why}"),
            RunError::Provision(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for RunError {}

impl From<ProvisionError> for RunError {
    fn from(error: ProvisionError) -> Self {
        Self::Provision(error)
    }
}

impl From<std::io::Error> for RunError {
    fn from(error: std::io::Error) -> Self {
        Self::Provision(ProvisionError::Io(error))
    }
}

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
    let mut base_url = None;
    let mut owner = None;
    let mut name = None;
    let mut out = None;
    let mut webhook_url = None;
    let mut webhook_secret_file = None;
    let mut iter = args.into_iter();
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--help" | "-h" => return Ok(ParseOutcome::Help),
            "--base-url" => base_url = Some(value_for(&flag, &mut iter)?),
            "--owner" => owner = Some(value_for(&flag, &mut iter)?),
            "--name" => name = Some(value_for(&flag, &mut iter)?),
            "--out" => out = Some(value_for(&flag, &mut iter)?),
            "--webhook-url" => webhook_url = Some(value_for(&flag, &mut iter)?),
            "--webhook-secret-file" => webhook_secret_file = Some(value_for(&flag, &mut iter)?),
            other => {
                return Err(ArgsError::new(format!(
                    "unrecognized argument '{other}'\nusage: {USAGE}"
                )))
            }
        }
    }
    let admin_token = env(ADMIN_TOKEN_ENV)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            ArgsError::new(format!(
                "missing required environment variable {ADMIN_TOKEN_ENV}"
            ))
        })?;
    Ok(ParseOutcome::Run(ProvisionArgs {
        base_url: require(base_url, "--base-url")?,
        owner: require(owner, "--owner")?,
        name: require(name, "--name")?,
        out: PathBuf::from(require(out, "--out")?),
        admin_token,
        webhook_url,
        webhook_secret_file: webhook_secret_file.map(PathBuf::from),
    }))
}

pub fn run(args: &ProvisionArgs) -> Result<String, RunError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| RunError::Runtime(err.to_string()))?;
    let (provisioned, issue) = runtime.block_on(provision::provision_and_seed(
        &args.base_url,
        &args.admin_token,
        &args.owner,
        &args.name,
        args.webhook_url.as_deref(),
        args.webhook_secret_file.as_deref(),
    ))?;
    provision::write_secrets_file(&args.out, &provision::format_secrets_env(&provisioned))?;
    Ok(format!(
        "provisioned {}/{}: {} role(s), intake issue #{}; secrets written to {}",
        provisioned.owner,
        provisioned.name,
        provisioned.roles.len(),
        issue,
        args.out.display(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_requires_admin_token_from_env() {
        let error = parse_with_env(
            [
                "--base-url",
                "http://127.0.0.1:3000",
                "--owner",
                "acme",
                "--name",
                "service",
                "--out",
                "roles.env",
            ]
            .into_iter()
            .map(String::from),
            |_| None,
        )
        .unwrap_err();
        assert!(error.to_string().contains(ADMIN_TOKEN_ENV));
    }

    #[test]
    fn parse_reads_token_from_env_and_debug_redacts_it() {
        let outcome = parse_with_env(
            [
                "--base-url",
                "http://127.0.0.1:3000",
                "--owner",
                "acme",
                "--name",
                "service",
                "--out",
                "roles.env",
            ]
            .into_iter()
            .map(String::from),
            |key| (key == ADMIN_TOKEN_ENV).then(|| "admin-secret".to_string()),
        )
        .expect("parses");
        let ParseOutcome::Run(args) = outcome else {
            panic!("expected run")
        };
        let rendered = format!("{args:?}");
        assert!(!rendered.contains("admin-secret"));
        assert!(rendered.contains("<redacted>"));
    }
}
