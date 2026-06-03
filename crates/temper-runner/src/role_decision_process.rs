//! Out-of-process workflow-role decision agent adapter.
//!
//! Temper serializes a versioned decision request, validates one reply from a
//! configured subprocess, and executes any authorized action only through
//! [`RoleTools`]. The subprocess receives no Forge handle or mutation tool.

use std::error::Error;
use std::fmt;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time;

use temper_forge::Forge;
use temper_workflow::{RoleManifest, ToolManifest};

use crate::role_process_tools::{build_work_item_context, run_process_action};
use crate::{
    redacted_lossy_preview, render_role_decision_reply_event, render_role_decision_request_event,
    Agent, AgentError, BoundExternalTool, ExternalToolExecutors, RoleDecisionReplyEvent,
    RoleDecisionRequestEvent, RoleTools, WorkItem, WorkflowRoleDecisionProtocolError,
    WorkflowRoleDecisionReply, WorkflowRoleDecisionRequest,
};

const STDERR_PREVIEW_LIMIT: usize = 4096;

/// Configuration for invoking an out-of-process workflow-role decision engine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkflowRoleDecisionProcessConfig {
    /// Program to execute.
    pub program: PathBuf,
    /// Arguments supplied to the program.
    pub args: Vec<String>,
    /// Optional working directory for the process.
    pub working_dir: Option<PathBuf>,
    /// Names of environment variables copied from Temper's process.
    pub env_allowlist: Vec<String>,
    /// Maximum wall-clock duration for one decision.
    pub timeout: Duration,
}

impl WorkflowRoleDecisionProcessConfig {
    /// Default one-decision timeout.
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

    /// Builds process configuration with no inherited environment and the
    /// default timeout.
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            working_dir: None,
            env_allowlist: Vec::new(),
            timeout: Self::DEFAULT_TIMEOUT,
        }
    }

    /// Replaces command arguments.
    pub fn with_args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }

    /// Sets the working directory.
    pub fn with_working_dir(mut self, working_dir: impl Into<PathBuf>) -> Self {
        self.working_dir = Some(working_dir.into());
        self
    }

    /// Replaces the environment-variable allow-list.
    pub fn with_env_allowlist<I, S>(mut self, names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.env_allowlist = names.into_iter().map(Into::into).collect();
        self
    }

    /// Sets the timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// Errors produced by the workflow-role decision process adapter before they
/// are mapped onto generic [`AgentError`] values.
#[derive(Debug)]
pub enum WorkflowRoleDecisionProcessError {
    /// Static process configuration is invalid.
    InvalidConfig {
        field: &'static str,
        message: String,
    },
    /// Spawning, writing to, or waiting for the process failed.
    Io {
        operation: &'static str,
        source: std::io::Error,
    },
    /// The process exceeded its timeout.
    Timeout { timeout: Duration },
    /// The process exited unsuccessfully.
    Exit { status: String, stderr: String },
    /// Stdout did not contain exactly one valid reply JSON value.
    MalformedJson { source: serde_json::Error },
    /// The reply did not satisfy the request contract.
    Protocol(WorkflowRoleDecisionProtocolError),
}

impl fmt::Display for WorkflowRoleDecisionProcessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig { field, message } => {
                write!(
                    formatter,
                    "invalid role decision process config `{field}`: {message}"
                )
            }
            Self::Io { operation, source } => {
                write!(
                    formatter,
                    "role decision process {operation} I/O failed: {source}"
                )
            }
            Self::Timeout { timeout } => {
                write!(
                    formatter,
                    "role decision process timed out after {timeout:?}"
                )
            }
            Self::Exit { status, stderr } => write!(
                formatter,
                "role decision process exited unsuccessfully with status {status}: {stderr}"
            ),
            Self::MalformedJson { source } => {
                write!(
                    formatter,
                    "role decision process returned malformed JSON: {source}"
                )
            }
            Self::Protocol(error) => write!(formatter, "{error}"),
        }
    }
}

impl Error for WorkflowRoleDecisionProcessError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::MalformedJson { source } => Some(source),
            Self::Protocol(error) => Some(error),
            Self::InvalidConfig { .. } | Self::Timeout { .. } | Self::Exit { .. } => None,
        }
    }
}

/// Agent adapter that obtains one workflow-role decision from a subprocess.
pub struct WorkflowRoleDecisionProcessAgent {
    workflow_id: String,
    manifest: RoleManifest,
    bound_external_tools: Vec<BoundExternalTool>,
    external_tool_executors: ExternalToolExecutors,
    config: WorkflowRoleDecisionProcessConfig,
}

impl WorkflowRoleDecisionProcessAgent {
    /// Builds a process-backed agent with no bound external-tool metadata.
    pub fn new(
        workflow_id: impl Into<String>,
        manifest: RoleManifest,
        config: WorkflowRoleDecisionProcessConfig,
    ) -> Result<Self, WorkflowRoleDecisionProcessError> {
        Self::with_bound_external_tools(workflow_id, manifest, config, Vec::new())
    }

    /// Builds a process-backed agent with runner-bound external-tool metadata.
    pub fn with_bound_external_tools(
        workflow_id: impl Into<String>,
        manifest: RoleManifest,
        config: WorkflowRoleDecisionProcessConfig,
        bound_external_tools: Vec<BoundExternalTool>,
    ) -> Result<Self, WorkflowRoleDecisionProcessError> {
        Self::with_bound_external_tools_and_executors(
            workflow_id,
            manifest,
            config,
            bound_external_tools,
            ExternalToolExecutors::new(),
        )
    }

    /// Builds a process-backed agent with metadata plus executable providers.
    pub fn with_bound_external_tools_and_executors(
        workflow_id: impl Into<String>,
        manifest: RoleManifest,
        config: WorkflowRoleDecisionProcessConfig,
        bound_external_tools: Vec<BoundExternalTool>,
        external_tool_executors: ExternalToolExecutors,
    ) -> Result<Self, WorkflowRoleDecisionProcessError> {
        validate_config(&config)?;
        let bound_external_tools = declared_bound_tools(&manifest, bound_external_tools);
        Ok(Self {
            workflow_id: workflow_id.into(),
            manifest,
            bound_external_tools,
            external_tool_executors,
            config,
        })
    }

    /// Returns the compiled role manifest this adapter enforces.
    pub fn manifest(&self) -> &RoleManifest {
        &self.manifest
    }

    /// Returns declared-and-bound external tools visible to the process.
    pub fn bound_external_tools(&self) -> &[BoundExternalTool] {
        &self.bound_external_tools
    }

    /// Returns the process configuration.
    pub fn config(&self) -> &WorkflowRoleDecisionProcessConfig {
        &self.config
    }

    /// Builds the exact versioned request that will be sent to the process.
    pub async fn build_request<F: Forge + ?Sized>(
        &self,
        item: &WorkItem,
        tools: &RoleTools<'_, F>,
    ) -> Result<WorkflowRoleDecisionRequest, AgentError> {
        Ok(WorkflowRoleDecisionRequest::new(
            self.workflow_id.clone(),
            self.manifest.clone(),
            build_work_item_context(item, tools).await?,
            self.bound_external_tools.clone(),
        ))
    }

    /// Invokes the configured process for one already-built request.
    pub async fn invoke(
        &self,
        request: &WorkflowRoleDecisionRequest,
    ) -> Result<WorkflowRoleDecisionReply, WorkflowRoleDecisionProcessError> {
        let reply = self.invoke_unvalidated(request).await?;
        request
            .validate_reply(&reply)
            .map_err(WorkflowRoleDecisionProcessError::Protocol)?;
        Ok(reply)
    }

    async fn invoke_unvalidated(
        &self,
        request: &WorkflowRoleDecisionRequest,
    ) -> Result<WorkflowRoleDecisionReply, WorkflowRoleDecisionProcessError> {
        let request_json = serde_json::to_vec(request)
            .map_err(|source| WorkflowRoleDecisionProcessError::MalformedJson { source })?;
        let mut command = Command::new(&self.config.program);
        command
            .args(&self.config.args)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(working_dir) = &self.config.working_dir {
            command.current_dir(working_dir);
        }
        for name in &self.config.env_allowlist {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }

        let mut child = command
            .spawn()
            .map_err(|source| WorkflowRoleDecisionProcessError::Io {
                operation: "spawn",
                source,
            })?;
        let mut stdin =
            child
                .stdin
                .take()
                .ok_or_else(|| WorkflowRoleDecisionProcessError::InvalidConfig {
                    field: "role_decision_process.stdin",
                    message: "process stdin unavailable".to_string(),
                })?;
        stdin.write_all(&request_json).await.map_err(|source| {
            WorkflowRoleDecisionProcessError::Io {
                operation: "write stdin",
                source,
            }
        })?;
        stdin
            .write_all(b"\n")
            .await
            .map_err(|source| WorkflowRoleDecisionProcessError::Io {
                operation: "write stdin",
                source,
            })?;
        drop(stdin);

        let output = time::timeout(self.config.timeout, child.wait_with_output())
            .await
            .map_err(|_| WorkflowRoleDecisionProcessError::Timeout {
                timeout: self.config.timeout,
            })?
            .map_err(|source| WorkflowRoleDecisionProcessError::Io {
                operation: "wait",
                source,
            })?;
        if !output.status.success() {
            return Err(WorkflowRoleDecisionProcessError::Exit {
                status: output.status.to_string(),
                stderr: preview_lossy(&output.stderr),
            });
        }
        serde_json::from_slice(&output.stdout)
            .map_err(|source| WorkflowRoleDecisionProcessError::MalformedJson { source })
    }

    fn tool_for_action(&self, action: &str) -> Option<&ToolManifest> {
        self.manifest.tools.iter().find(|tool| tool.name == action)
    }
}

#[async_trait]
impl<F: Forge + ?Sized> Agent<F> for WorkflowRoleDecisionProcessAgent {
    async fn service(&self, item: &WorkItem, tools: &RoleTools<'_, F>) -> Result<bool, AgentError> {
        let request = self.build_request(item, tools).await?;
        let identity = tools.work_item_identity(item);
        log_decision_request(&identity, &request);
        let started = Instant::now();
        let reply = match self.invoke_unvalidated(&request).await {
            Ok(reply) => reply,
            Err(error) => {
                let (validation_outcome, action_kind) = classify_process_error(&error);
                let error_message = error.to_string();
                log_decision_reply(
                    &identity,
                    None,
                    validation_outcome,
                    action_kind,
                    None,
                    started.elapsed(),
                    Some(error_message.as_str()),
                );
                return Err(AgentError::message(error_message));
            }
        };

        let classification = classify_decision_reply(&request, &reply);
        log_decision_reply(
            &identity,
            Some(reply.action.as_str()),
            classification.validation_outcome,
            classification.action_kind,
            Some(reply.reason.as_str()),
            started.elapsed(),
            classification.error.as_deref(),
        );

        match classification.disposition {
            DecisionDisposition::ExecuteAction => {
                let Some(tool) = self.tool_for_action(&reply.action) else {
                    return Ok(false);
                };
                run_process_action(
                    &self.manifest,
                    &self.bound_external_tools,
                    &self.external_tool_executors,
                    item,
                    tools,
                    tool,
                    &request.work_item_context,
                )
                .await
            }
            DecisionDisposition::NoAction => Ok(false),
            DecisionDisposition::Error => {
                Err(AgentError::message(classification.error.unwrap_or_else(
                    || "role decision reply failed validation".to_string(),
                )))
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DecisionDisposition {
    ExecuteAction,
    NoAction,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DecisionReplyClassification {
    validation_outcome: &'static str,
    action_kind: &'static str,
    disposition: DecisionDisposition,
    error: Option<String>,
}

fn classify_decision_reply(
    request: &WorkflowRoleDecisionRequest,
    reply: &WorkflowRoleDecisionReply,
) -> DecisionReplyClassification {
    if reply.protocol_version != request.protocol_version {
        let error = WorkflowRoleDecisionProtocolError::VersionMismatch {
            expected: request.protocol_version,
            actual: reply.protocol_version,
        };
        return DecisionReplyClassification {
            validation_outcome: "protocol_mismatch",
            action_kind: "invalid_reply",
            disposition: DecisionDisposition::Error,
            error: Some(error.to_string()),
        };
    }
    if !request.action_is_authorized(&reply.action) {
        return DecisionReplyClassification {
            validation_outcome: "unauthorized_downgraded_to_no_action",
            action_kind: "no_action",
            disposition: DecisionDisposition::NoAction,
            error: None,
        };
    }
    if reply.is_no_action() {
        return DecisionReplyClassification {
            validation_outcome: "valid",
            action_kind: "no_action",
            disposition: DecisionDisposition::NoAction,
            error: None,
        };
    }
    DecisionReplyClassification {
        validation_outcome: "valid",
        action_kind: "authorized_action",
        disposition: DecisionDisposition::ExecuteAction,
        error: None,
    }
}

fn classify_process_error(
    error: &WorkflowRoleDecisionProcessError,
) -> (&'static str, &'static str) {
    match error {
        WorkflowRoleDecisionProcessError::MalformedJson { .. } => {
            ("malformed_json", "invalid_reply")
        }
        WorkflowRoleDecisionProcessError::Timeout { .. } => ("timeout", "process_unavailable"),
        WorkflowRoleDecisionProcessError::Protocol(
            WorkflowRoleDecisionProtocolError::VersionMismatch { .. },
        ) => ("protocol_mismatch", "invalid_reply"),
        WorkflowRoleDecisionProcessError::Protocol(
            WorkflowRoleDecisionProtocolError::UnauthorizedAction { .. },
        ) => ("unauthorized_downgraded_to_no_action", "no_action"),
        WorkflowRoleDecisionProcessError::InvalidConfig { .. }
        | WorkflowRoleDecisionProcessError::Io { .. }
        | WorkflowRoleDecisionProcessError::Exit { .. } => {
            ("process_failure", "process_unavailable")
        }
    }
}

fn log_decision_request(identity: &crate::WorkItemIdentity, request: &WorkflowRoleDecisionRequest) {
    let authorized_actions = request
        .authorized_actions
        .iter()
        .map(|action| action.action.clone())
        .collect::<Vec<_>>();
    let available_external_tools = request
        .available_external_tools
        .iter()
        .map(|tool| tool.id.to_string())
        .collect::<Vec<_>>();
    eprintln!(
        "{}",
        render_role_decision_request_event(&RoleDecisionRequestEvent {
            identity,
            workflow_id: &request.workflow_id,
            authorized_actions: &authorized_actions,
            available_external_tools: &available_external_tools,
        })
    );
}

fn log_decision_reply(
    identity: &crate::WorkItemIdentity,
    selected_action: Option<&str>,
    validation_outcome: &str,
    action_kind: &str,
    reason: Option<&str>,
    latency: Duration,
    error: Option<&str>,
) {
    eprintln!(
        "{}",
        render_role_decision_reply_event(&RoleDecisionReplyEvent {
            identity,
            selected_action,
            validation_outcome,
            action_kind,
            reason,
            latency,
            error,
        })
    );
}

fn validate_config(
    config: &WorkflowRoleDecisionProcessConfig,
) -> Result<(), WorkflowRoleDecisionProcessError> {
    if config.program.as_os_str().is_empty() {
        return Err(WorkflowRoleDecisionProcessError::InvalidConfig {
            field: "role_decision_process.program",
            message: "must not be empty".to_string(),
        });
    }
    if config.timeout.is_zero() {
        return Err(WorkflowRoleDecisionProcessError::InvalidConfig {
            field: "role_decision_process.timeout",
            message: "must be greater than zero".to_string(),
        });
    }
    for name in &config.env_allowlist {
        validate_env_name(name)?;
    }
    Ok(())
}

fn validate_env_name(name: &str) -> Result<(), WorkflowRoleDecisionProcessError> {
    if name.is_empty() || name.contains('=') || name.contains('\0') {
        return Err(WorkflowRoleDecisionProcessError::InvalidConfig {
            field: "role_decision_process.env_allowlist",
            message: format!("invalid environment variable name `{name}`"),
        });
    }
    Ok(())
}

fn preview_lossy(bytes: &[u8]) -> String {
    redacted_lossy_preview(bytes, STDERR_PREVIEW_LIMIT)
}

fn declared_bound_tools(
    manifest: &RoleManifest,
    bound_external_tools: Vec<BoundExternalTool>,
) -> Vec<BoundExternalTool> {
    manifest
        .external_tools
        .iter()
        .filter_map(|declared| {
            bound_external_tools
                .iter()
                .find(|tool| tool.id == declared.id)
                .cloned()
        })
        .collect()
}

#[cfg(test)]
#[path = "role_decision_process_tests.rs"]
mod tests;
