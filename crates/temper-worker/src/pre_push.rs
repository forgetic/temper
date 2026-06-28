use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use temper_protocol_agent::{
    PROTOCOL_VERSION, SubmitForPrGate, SubmitForPrRequest, SubmitForPrResponse, WorkspaceContext,
};

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

/// Runs the worker-owned pre-push checks for a live `submit_for_pr` attempt.
///
/// The workspace root is the agent cwd containing one sibling directory per
/// repository from [`WorkspaceContext::repos`]. Each writable repo is checked in
/// its own current checkout, so a newly edited `.temper/pre-push.toml` is the
/// config that is evaluated. Missing configs are accepted as a no-op.
pub async fn submit_for_pr_pre_push_response(
    request: &SubmitForPrRequest,
    context: &WorkspaceContext,
    workspace_root: impl AsRef<Path>,
) -> SubmitForPrResponse {
    let reports = match run_workspace_pre_push_checks(context, workspace_root.as_ref()).await {
        Ok(reports) => reports,
        Err(error) => {
            return SubmitForPrResponse::rejected(format!(
                "pre-push checks could not run for {}: {error}",
                request.correlation_key
            ));
        }
    };

    response_from_reports(&request.correlation_key, reports)
}

/// Synchronous wrapper for the out-of-process `submit_for_pr` side-channel
/// thread. It runs the async checker on a short-lived worker runtime so the
/// child agent receives a normal structured tool response on the same request.
pub fn submit_for_pr_pre_push_response_blocking(
    request: SubmitForPrRequest,
    context: &WorkspaceContext,
    workspace_root: &Path,
) -> SubmitForPrResponse {
    let context = context.clone();
    let workspace_root = workspace_root.to_path_buf();
    temper_worker_io::block_on(async move {
        submit_for_pr_pre_push_response(&request, &context, &workspace_root).await
    })
}

/// Runs the same checks defensively on the terminal success path, preventing an
/// agent from bypassing configured gates by skipping `submit_for_pr` and ending
/// with final JSON.
pub async fn final_pre_push_response(
    context: &WorkspaceContext,
    workspace_root: impl AsRef<Path>,
) -> SubmitForPrResponse {
    let request = SubmitForPrRequest {
        protocol_version: PROTOCOL_VERSION,
        correlation_key: context.correlation_key.clone(),
        role: context.work_item.role.clone(),
        action: context.action.clone(),
        summary: None,
    };
    submit_for_pr_pre_push_response(&request, context, workspace_root).await
}

struct RepoPrePushReport {
    report: PrePushReport,
}

async fn run_workspace_pre_push_checks(
    context: &WorkspaceContext,
    workspace_root: &Path,
) -> Result<Vec<RepoPrePushReport>, PrePushError> {
    let mut reports = Vec::new();
    for repo in context.repos.iter().filter(|repo| repo.is_writable()) {
        let report = run_pre_push_checks(workspace_root.join(&repo.dir)).await?;
        reports.push(RepoPrePushReport { report });
    }
    Ok(reports)
}

fn response_from_reports(
    correlation_key: &str,
    reports: Vec<RepoPrePushReport>,
) -> SubmitForPrResponse {
    let configured = reports
        .iter()
        .filter(|repo| repo.report.status != PrePushStatus::NotConfigured)
        .count();
    let accepted = reports.iter().all(|repo| repo.report.passed());
    let gates = reports
        .iter()
        .flat_map(|repo| repo.report.commands.iter().map(gate))
        .collect::<Vec<_>>();

    if configured == 0 {
        return SubmitForPrResponse {
            accepted: true,
            message: format!(
                "host accepted submit_for_pr for {correlation_key}; no pre-push gates configured"
            ),
            gates,
        };
    }

    if accepted {
        SubmitForPrResponse {
            accepted: true,
            message: format!(
                "pre-push gates passed for {configured} writable repo(s) in {correlation_key}"
            ),
            gates,
        }
    } else {
        SubmitForPrResponse {
            accepted: false,
            message: format!(
                "pre-push gates failed for {correlation_key}; fix the reported command output and submit_for_pr again"
            ),
            gates,
        }
    }
}

fn gate(command: &PrePushCommandResult) -> SubmitForPrGate {
    SubmitForPrGate {
        command_id: format!("pre-push:{}", command.id),
        argv: command.argv.clone(),
        cwd: command.cwd.display().to_string(),
        exit_status: command_status(command),
        exit_code: command.exit_code,
        stdout_tail: command.stdout_tail.clone(),
        stderr_tail: command.stderr_tail.clone(),
        timed_out: command.timed_out,
        elapsed_ms: command.elapsed_ms,
    }
}

fn command_status(command: &PrePushCommandResult) -> String {
    if command.timed_out {
        "timeout".to_string()
    } else if command.succeeded() {
        "passed".to_string()
    } else if command.error.is_some() {
        "error".to_string()
    } else {
        "failed".to_string()
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
