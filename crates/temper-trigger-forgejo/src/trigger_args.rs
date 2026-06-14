//! Argument parsing and top-level runner for `temper-trigger-forgejo`.

use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;

use crate::trigger::{self, TriggerError};

pub const USAGE: &str = concat!(
    "temper-trigger-forgejo --bind <addr:port> --webhook-secret-file <path> ",
    "(--wake-dir <path> | --wake-socket <name=path> [...]) ",
    "[--wake-secret-file <path>]\n",
    "  verifies Forgejo/Gitea webhook signatures and sends local worker wakes"
);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseOutcome {
    Run(TriggerArgs),
    Help,
}

#[derive(Clone, Eq, PartialEq)]
pub struct TriggerArgs {
    pub bind: SocketAddr,
    pub webhook_secret_file: PathBuf,
    pub wake_secret_file: Option<PathBuf>,
    pub wake_dir: Option<PathBuf>,
    pub wake_sockets: Vec<NamedSocket>,
}

impl fmt::Debug for TriggerArgs {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TriggerArgs")
            .field("bind", &self.bind)
            .field("webhook_secret_file", &self.webhook_secret_file)
            .field("wake_secret_file", &self.wake_secret_file)
            .field("wake_dir", &self.wake_dir)
            .field("wake_sockets", &self.wake_sockets)
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NamedSocket {
    pub name: String,
    pub path: PathBuf,
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
    Trigger(TriggerError),
}

impl fmt::Display for RunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::Trigger(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for RunError {}

impl From<TriggerError> for RunError {
    fn from(error: TriggerError) -> Self {
        Self::Trigger(error)
    }
}

pub fn parse<I>(args: I) -> Result<ParseOutcome, ArgsError>
where
    I: IntoIterator<Item = String>,
{
    let mut bind = None;
    let mut webhook_secret_file = None;
    let mut wake_secret_file = None;
    let mut wake_dir = None;
    let mut wake_sockets = Vec::new();
    let mut iter = args.into_iter();
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--help" | "-h" => return Ok(ParseOutcome::Help),
            "--bind" => bind = Some(value_for(&flag, &mut iter)?),
            "--webhook-secret-file" => webhook_secret_file = Some(value_for(&flag, &mut iter)?),
            "--wake-secret-file" => wake_secret_file = Some(value_for(&flag, &mut iter)?),
            "--wake-dir" => wake_dir = Some(value_for(&flag, &mut iter)?),
            "--wake-socket" => {
                wake_sockets.push(parse_named_socket(&value_for(&flag, &mut iter)?)?)
            }
            other => {
                return Err(ArgsError::new(format!(
                    "unrecognized argument '{other}'\nusage: {USAGE}"
                )));
            }
        }
    }
    if wake_sockets.is_empty() && wake_dir.is_none() {
        return Err(ArgsError::new(format!(
            "missing --wake-dir or at least one --wake-socket\nusage: {USAGE}"
        )));
    }
    Ok(ParseOutcome::Run(TriggerArgs {
        bind: require(bind, "--bind")?
            .parse()
            .map_err(|_| ArgsError::new("--bind must be addr:port"))?,
        webhook_secret_file: PathBuf::from(require(webhook_secret_file, "--webhook-secret-file")?),
        wake_secret_file: wake_secret_file.map(PathBuf::from),
        wake_dir: wake_dir.map(PathBuf::from),
        wake_sockets,
    }))
}

pub fn run(args: &TriggerArgs) -> Result<(), RunError> {
    trigger::run(args).map_err(Into::into)
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

fn parse_named_socket(raw: &str) -> Result<NamedSocket, ArgsError> {
    let (name, path) = raw
        .split_once('=')
        .ok_or_else(|| ArgsError::new("--wake-socket must be name=path"))?;
    if name.is_empty() || path.is_empty() {
        return Err(ArgsError::new(
            "--wake-socket requires non-empty name and path",
        ));
    }
    Ok(NamedSocket {
        name: name.to_string(),
        path: PathBuf::from(path),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_trigger_args() {
        let outcome = parse(
            [
                "--bind",
                "127.0.0.1:4010",
                "--webhook-secret-file",
                "secrets/webhook",
                "--wake-secret-file",
                "secrets/wake",
                "--wake-dir",
                "run/wake",
            ]
            .into_iter()
            .map(String::from),
        )
        .expect("parses");
        let ParseOutcome::Run(args) = outcome else {
            panic!("expected run")
        };
        assert_eq!(args.bind.to_string(), "127.0.0.1:4010");
        assert_eq!(args.wake_dir, Some(PathBuf::from("run/wake")));
    }

    #[test]
    fn requires_socket() {
        let error = parse(
            [
                "--bind",
                "127.0.0.1:4010",
                "--webhook-secret-file",
                "secrets/webhook",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap_err();
        assert!(error.to_string().contains("--wake-dir"));
    }
}
