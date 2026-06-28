use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

mod process;

pub use process::PrePushCommandResult;
use process::run_command;

/// Runs repository-configured pre-push checks from `.temper/pre-push.toml`.
///
/// The supplied path must be the writable repository checkout root. A missing
/// config file is a successful no-op (`PrePushStatus::NotConfigured`) so callers
/// can opt into this runner without changing existing repositories.
pub async fn run_pre_push_checks(
    repo_root: impl AsRef<Path>,
) -> Result<PrePushReport, PrePushError> {
    let repo_root = repo_root.as_ref().to_path_buf();
    let config_path = repo_root.join(".temper").join("pre-push.toml");
    let Some(plan) = load_plan(&config_path)? else {
        return Ok(PrePushReport {
            config_path,
            status: PrePushStatus::NotConfigured,
            required: false,
            commands: Vec::new(),
        });
    };

    let cwd = plan.cwd.resolve(&repo_root);
    let mut report = PrePushReport {
        config_path,
        status: PrePushStatus::Passed,
        required: plan.required,
        commands: Vec::new(),
    };

    for command in plan.commands {
        let outcome = run_command(command, cwd.clone()).await;
        let succeeded = outcome.succeeded();
        report.commands.push(outcome);
        if !succeeded {
            report.status = PrePushStatus::Failed;
            break;
        }
    }

    Ok(report)
}

/// Structured result for one pre-push config evaluation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PrePushReport {
    /// Path checked for the repository-owned config.
    pub config_path: PathBuf,
    /// Overall outcome for the configured command sequence.
    pub status: PrePushStatus,
    /// The `pre_push.required` flag from the config, or `false` when missing.
    pub required: bool,
    /// Command outcomes in execution order. Stops at the first failure/timeout.
    pub commands: Vec<PrePushCommandResult>,
}

impl PrePushReport {
    /// Whether the configured checks completed without a command failure.
    /// Missing config returns true to preserve current behavior.
    pub fn passed(&self) -> bool {
        matches!(
            self.status,
            PrePushStatus::NotConfigured | PrePushStatus::Passed
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrePushStatus {
    NotConfigured,
    Passed,
    Failed,
}

#[derive(Debug, thiserror::Error)]
pub enum PrePushError {
    #[error("read pre-push config `{path}`: {source}")]
    ReadConfig {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("parse pre-push config `{path}`: {source}")]
    ParseConfig {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("unsupported pre-push config version {version}; expected 1")]
    UnsupportedVersion { version: u64 },
    #[error("unsupported pre-push cwd `{cwd}`; expected `repo`")]
    UnsupportedCwd { cwd: String },
    #[error("pre-push command at index {index} has an empty id")]
    EmptyCommandId { index: usize },
    #[error("pre-push command `{id}` has an empty argv")]
    EmptyArgv { id: String },
    #[error("pre-push command `{id}` has an empty program")]
    EmptyProgram { id: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PrePushPlan {
    required: bool,
    cwd: PrePushCwd,
    commands: Vec<PrePushCommand>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum PrePushCwd {
    Repo,
}

impl PrePushCwd {
    fn parse(value: String) -> Result<Self, PrePushError> {
        match value.as_str() {
            "repo" => Ok(Self::Repo),
            _ => Err(PrePushError::UnsupportedCwd { cwd: value }),
        }
    }

    fn resolve(&self, repo_root: &Path) -> PathBuf {
        match self {
            Self::Repo => repo_root.to_path_buf(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PrePushCommand {
    pub(super) id: String,
    pub(super) argv: Vec<String>,
    pub(super) timeout_secs: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    version: u64,
    pre_push: RawPrePush,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPrePush {
    required: bool,
    cwd: String,
    #[serde(default)]
    commands: Vec<RawCommand>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCommand {
    id: String,
    argv: Vec<String>,
    timeout_secs: u64,
}

fn load_plan(config_path: &Path) -> Result<Option<PrePushPlan>, PrePushError> {
    let text = match fs::read_to_string(config_path) {
        Ok(text) => text,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(PrePushError::ReadConfig {
                path: config_path.to_path_buf(),
                source,
            });
        }
    };
    let raw = toml::from_str::<RawConfig>(&text).map_err(|source| PrePushError::ParseConfig {
        path: config_path.to_path_buf(),
        source,
    })?;
    plan_from_raw(raw).map(Some)
}

fn plan_from_raw(raw: RawConfig) -> Result<PrePushPlan, PrePushError> {
    if raw.version != 1 {
        return Err(PrePushError::UnsupportedVersion {
            version: raw.version,
        });
    }
    let cwd = PrePushCwd::parse(raw.pre_push.cwd)?;
    let commands = raw
        .pre_push
        .commands
        .into_iter()
        .enumerate()
        .map(|(index, command)| command_from_raw(index, command))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(PrePushPlan {
        required: raw.pre_push.required,
        cwd,
        commands,
    })
}

fn command_from_raw(index: usize, raw: RawCommand) -> Result<PrePushCommand, PrePushError> {
    let id = raw.id.trim().to_string();
    if id.is_empty() {
        return Err(PrePushError::EmptyCommandId { index });
    }
    if raw.argv.is_empty() {
        return Err(PrePushError::EmptyArgv { id });
    }
    if raw.argv[0].trim().is_empty() {
        return Err(PrePushError::EmptyProgram { id });
    }
    Ok(PrePushCommand {
        id,
        argv: raw.argv,
        timeout_secs: raw.timeout_secs,
    })
}
