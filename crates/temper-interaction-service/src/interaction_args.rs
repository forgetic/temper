//! Argument parsing for the generic interaction service binary.

use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;

use temper_interaction::ConversationProfileId;

pub const USAGE: &str = concat!(
    "temper-interaction repl --spec <path> --bindings <path> --profile <id> ",
    "[--transcript-issue <n>]\n",
    "temper-interaction serve --spec <path> --bindings <path> ",
    "[--bind <addr:port>] [--allow-non-loopback]\n",
    "  --spec, --bindings, and --profile are required CLI flags. Spec files define ",
    "profile behavior. Deployment bindings name Forge token environment variables, ",
    "responder processes, repository, bind address, and optional service token env. ",
    "Secrets are read from env/config, never argv."
);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseOutcome {
    Repl(Box<InteractionReplArgs>),
    Serve(Box<InteractionServeArgs>),
    Help,
}

#[derive(Clone, Eq, PartialEq)]
pub struct InteractionReplArgs {
    pub spec_path: PathBuf,
    pub bindings_path: PathBuf,
    pub profile_id: ConversationProfileId,
    pub transcript_issue: Option<u64>,
}

impl fmt::Debug for InteractionReplArgs {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InteractionReplArgs")
            .field("spec_path", &self.spec_path)
            .field("bindings_path", &self.bindings_path)
            .field("profile_id", &self.profile_id)
            .field("transcript_issue", &self.transcript_issue)
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct InteractionServeArgs {
    pub spec_path: PathBuf,
    pub bindings_path: PathBuf,
    pub bind_override: Option<SocketAddr>,
    pub allow_non_loopback: bool,
}

impl fmt::Debug for InteractionServeArgs {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InteractionServeArgs")
            .field("spec_path", &self.spec_path)
            .field("bindings_path", &self.bindings_path)
            .field("bind_override", &self.bind_override)
            .field("allow_non_loopback", &self.allow_non_loopback)
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

/// Parses the interaction-service CLI arguments. All inputs come from flags;
/// `--spec`, `--bindings`, and `--profile` are required (no environment
/// fallback). Secrets are still resolved from the environment elsewhere via the
/// env-var names the bindings file declares — never from argv.
pub fn parse<I>(args: I) -> Result<ParseOutcome, ArgsError>
where
    I: IntoIterator<Item = String>,
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
            if raw.common.help {
                Ok(ParseOutcome::Help)
            } else {
                raw.into_args()
                    .map(|args| ParseOutcome::Repl(Box::new(args)))
            }
        }
        "serve" => {
            let raw = RawServeArgs::collect(iter)?;
            if raw.common.help {
                Ok(ParseOutcome::Help)
            } else {
                raw.into_args()
                    .map(|args| ParseOutcome::Serve(Box::new(args)))
            }
        }
        other => Err(ArgsError::new(format!(
            "unknown subcommand '{other}'; expected repl or serve\nusage: {USAGE}"
        ))),
    }
}

#[derive(Default)]
struct RawCommonArgs {
    help: bool,
    spec_path: Option<String>,
    bindings_path: Option<String>,
}

impl RawCommonArgs {
    fn into_paths(self) -> Result<(PathBuf, PathBuf), ArgsError> {
        let spec = non_empty(self.spec_path)
            .ok_or_else(|| ArgsError::new("missing required --spec".to_string()))?;
        let bindings = non_empty(self.bindings_path)
            .ok_or_else(|| ArgsError::new("missing required --bindings".to_string()))?;
        Ok((PathBuf::from(spec), PathBuf::from(bindings)))
    }
}

#[derive(Default)]
struct RawReplArgs {
    common: RawCommonArgs,
    profile_id: Option<String>,
    transcript_issue: Option<String>,
}

impl RawReplArgs {
    fn collect<I>(args: I) -> Result<Self, ArgsError>
    where
        I: IntoIterator<Item = String>,
    {
        let mut raw = Self::default();
        let mut iter = args.into_iter();
        while let Some(flag) = iter.next() {
            match flag.as_str() {
                "--help" | "-h" => raw.common.help = true,
                "--spec" => raw.common.spec_path = Some(value_for(&flag, &mut iter)?),
                "--bindings" => raw.common.bindings_path = Some(value_for(&flag, &mut iter)?),
                "--profile" => raw.profile_id = Some(value_for(&flag, &mut iter)?),
                "--transcript-issue" => raw.transcript_issue = Some(value_for(&flag, &mut iter)?),
                other => {
                    return Err(ArgsError::new(format!(
                        "unrecognized argument '{other}'\nusage: {USAGE}"
                    )));
                }
            }
        }
        Ok(raw)
    }

    fn into_args(self) -> Result<InteractionReplArgs, ArgsError> {
        let (spec_path, bindings_path) = self.common.into_paths()?;
        let profile = non_empty(self.profile_id)
            .ok_or_else(|| ArgsError::new("missing required --profile".to_string()))?;
        Ok(InteractionReplArgs {
            spec_path,
            bindings_path,
            profile_id: ConversationProfileId::new(profile)
                .map_err(|error| ArgsError::new(error.to_string()))?,
            transcript_issue: self
                .transcript_issue
                .map(|raw| parse_issue_number(&raw, "--transcript-issue"))
                .transpose()?,
        })
    }
}

#[derive(Default)]
struct RawServeArgs {
    common: RawCommonArgs,
    bind: Option<String>,
    allow_non_loopback: bool,
}

impl RawServeArgs {
    fn collect<I>(args: I) -> Result<Self, ArgsError>
    where
        I: IntoIterator<Item = String>,
    {
        let mut raw = Self::default();
        let mut iter = args.into_iter();
        while let Some(flag) = iter.next() {
            match flag.as_str() {
                "--help" | "-h" => raw.common.help = true,
                "--spec" => raw.common.spec_path = Some(value_for(&flag, &mut iter)?),
                "--bindings" => raw.common.bindings_path = Some(value_for(&flag, &mut iter)?),
                "--bind" => raw.bind = Some(value_for(&flag, &mut iter)?),
                "--allow-non-loopback" => raw.allow_non_loopback = true,
                other => {
                    return Err(ArgsError::new(format!(
                        "unrecognized argument '{other}'\nusage: {USAGE}"
                    )));
                }
            }
        }
        Ok(raw)
    }

    fn into_args(self) -> Result<InteractionServeArgs, ArgsError> {
        let (spec_path, bindings_path) = self.common.into_paths()?;
        Ok(InteractionServeArgs {
            spec_path,
            bindings_path,
            bind_override: self
                .bind
                .map(|raw| parse_bind(&raw, "--bind"))
                .transpose()?,
            allow_non_loopback: self.allow_non_loopback,
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

fn non_empty(value: Option<String>) -> Option<String> {
    value.and_then(|value| (!value.trim().is_empty()).then_some(value))
}

fn parse_bind(raw: &str, flag: &str) -> Result<SocketAddr, ArgsError> {
    raw.parse()
        .map_err(|_| ArgsError::new(format!("{flag} must be addr:port, got '{raw}'")))
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
