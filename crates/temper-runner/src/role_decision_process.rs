//! Out-of-process workflow-role decision agent adapter.
//!
//! Temper serializes a versioned decision request, validates one reply from a
//! configured subprocess, and executes any authorized action only through
//! [`RoleTools`]. The subprocess receives no Forge handle or mutation tool.

mod classify;
mod config;
mod error;

use std::time::Instant;

use async_trait::async_trait;
use temper_engine_io::process::{ProcessCall, ProcessCallError, run_process};

use temper_forge::Forge;
use temper_workflow::{RoleManifest, ToolManifest};

use crate::role_decision::workflow_role_manifest_from_runtime;
use crate::role_process_tools::{build_work_item_context, run_process_action};
use crate::{
    Agent, AgentError, BoundExternalTool, ExternalToolExecutors, RoleTools, WorkItem,
    WorkflowRoleDecisionReply, WorkflowRoleDecisionRequest,
};
use temper_log::redact::redacted_lossy_preview;

use classify::{
    DecisionDisposition, classify_decision_reply, classify_process_error, log_decision_reply,
    log_decision_request,
};
pub use config::WorkflowRoleDecisionProcessConfig;
pub use error::WorkflowRoleDecisionProcessError;

use config::validate_config;

const STDERR_PREVIEW_LIMIT: usize = 4096;

/// Agent adapter that obtains one workflow-role decision from a subprocess.
pub struct WorkflowRoleDecisionProcessAgent {
    /// Clock/deadline capability of the engine task this agent runs under,
    /// injected at construction — process timeouts are computed against it.
    cx: temper_engine_io::Cx,
    workflow_id: String,
    manifest: RoleManifest,
    bound_external_tools: Vec<BoundExternalTool>,
    external_tool_executors: ExternalToolExecutors,
    config: WorkflowRoleDecisionProcessConfig,
}

impl WorkflowRoleDecisionProcessAgent {
    /// Builds a process-backed agent with no bound external-tool metadata.
    pub fn new(
        cx: temper_engine_io::Cx,
        workflow_id: impl Into<String>,
        manifest: RoleManifest,
        config: WorkflowRoleDecisionProcessConfig,
    ) -> Result<Self, WorkflowRoleDecisionProcessError> {
        Self::with_bound_external_tools(cx, workflow_id, manifest, config, Vec::new())
    }

    /// Builds a process-backed agent with runner-bound external-tool metadata.
    pub fn with_bound_external_tools(
        cx: temper_engine_io::Cx,
        workflow_id: impl Into<String>,
        manifest: RoleManifest,
        config: WorkflowRoleDecisionProcessConfig,
        bound_external_tools: Vec<BoundExternalTool>,
    ) -> Result<Self, WorkflowRoleDecisionProcessError> {
        Self::with_bound_external_tools_and_executors(
            cx,
            workflow_id,
            manifest,
            config,
            bound_external_tools,
            ExternalToolExecutors::new(),
        )
    }

    /// Builds a process-backed agent with metadata plus executable providers.
    pub fn with_bound_external_tools_and_executors(
        cx: temper_engine_io::Cx,
        workflow_id: impl Into<String>,
        manifest: RoleManifest,
        config: WorkflowRoleDecisionProcessConfig,
        bound_external_tools: Vec<BoundExternalTool>,
        external_tool_executors: ExternalToolExecutors,
    ) -> Result<Self, WorkflowRoleDecisionProcessError> {
        validate_config(&config)?;
        let bound_external_tools = declared_bound_tools(&manifest, bound_external_tools);
        Ok(Self {
            cx,
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
            workflow_role_manifest_from_runtime(&self.manifest),
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
        let mut request_json = serde_json::to_vec(request)
            .map_err(|source| WorkflowRoleDecisionProcessError::MalformedJson { source })?;
        request_json.push(b'\n');

        // One subprocess decision is one `<io-event-request>`: the call below
        // is data describing the spawn (program, args, allow-listed env,
        // stdin payload, deadline); the engine performs it and hands back the
        // buffered output for this pure code to interpret.
        let mut call = ProcessCall::new(self.config.program.to_string_lossy().into_owned());
        call.args = self.config.args.clone();
        call.clear_env = true;
        call.current_dir = self.config.working_dir.clone();
        call.stdin = Some(request_json);
        call.timeout = Some(self.config.timeout);
        for (name, value) in &self.config.env {
            call.env.insert(name.clone(), value.clone());
        }

        let output = run_process(&self.cx, call)
            .await
            .map_err(|error| match error {
                ProcessCallError::Spawn(message) => WorkflowRoleDecisionProcessError::Io {
                    operation: "spawn",
                    source: std::io::Error::other(message),
                },
                ProcessCallError::Io { operation, message } => {
                    WorkflowRoleDecisionProcessError::Io {
                        operation,
                        source: std::io::Error::other(message),
                    }
                }
                ProcessCallError::TimedOut => WorkflowRoleDecisionProcessError::Timeout {
                    timeout: self.config.timeout,
                },
            })?;
        if !output.success {
            return Err(WorkflowRoleDecisionProcessError::Exit {
                status: output.status_display,
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
                .find(|tool| tool.id == declared.id.as_str())
                .cloned()
        })
        .collect()
}

#[cfg(test)]
#[path = "role_decision_process_tests.rs"]
mod tests;
