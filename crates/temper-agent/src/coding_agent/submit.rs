//! Host-controlled `submit_for_pr` tool plumbing.
//!
//! The tool is intentionally a relay: it packages a small
//! [`SubmitForPrRequest`], invokes the host callback that the run path supplied,
//! and returns the host's [`SubmitForPrResponse`] to the same live model turn as
//! an ordinary tool result. It does not run git, run checks, push, or decide that
//! a workspace is ready.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use temper_protocol_agent::{
    PROTOCOL_VERSION, SubmitForPrGate, SubmitForPrRequest, SubmitForPrResponse, WorkspaceContext,
};
use tongs::error::{Error, Result};
use tongs::model::{ContentBlock, TextContent};
use tongs::tools::{Tool, ToolEffects, ToolOutput, ToolUpdate};

/// Agent-side callback invoked by the tool. Native in-process hosts bind this
/// from a host callback; the out-of-process agent binds it to the worker-owned
/// local side channel.
pub type SubmitForPrFuture = Pin<Box<dyn Future<Output = SubmitForPrResponse> + Send + 'static>>;
pub type SubmitForPrCallback = Arc<dyn Fn(SubmitForPrRequest) -> SubmitForPrFuture + Send + Sync>;

/// Host-side submit gate callback. It receives the tool request plus the run's
/// immutable context and prepared workspace root, so future implementations can
/// run pre-push checks from the host/worker layer without moving that decision
/// into the model/tool code.
pub type SubmitForPrHost =
    Arc<dyn Fn(SubmitForPrRequest, WorkspaceContext, PathBuf) -> SubmitForPrFuture + Send + Sync>;

/// Host gate used when a caller has not installed real checks yet. It preserves
/// the host-controlled shape while leaving actual pre-push enforcement to the
/// follow-up work item.
pub fn default_submit_for_pr_host() -> SubmitForPrHost {
    Arc::new(|request, _context, _cwd| {
        Box::pin(async move {
            SubmitForPrResponse::accepted(format!(
                "host accepted submit_for_pr for {}; no submit gates are configured yet",
                request.correlation_key
            ))
        })
    })
}

/// Binds a host callback to one run's context/cwd, yielding the agent-side
/// callback captured by the tool.
pub fn bind_submit_for_pr_host(
    host: SubmitForPrHost,
    context: &WorkspaceContext,
    cwd: &Path,
) -> SubmitForPrCallback {
    let context = context.clone();
    let cwd = cwd.to_path_buf();
    Arc::new(move |request| host(request, context.clone(), cwd.clone()))
}

/// Whether this workspace turn is allowed to expose `submit_for_pr`.
pub fn submit_for_pr_available(context: &WorkspaceContext) -> bool {
    context.work_item.role == "engineer"
        && context.repos.iter().any(|repo| repo.is_writable())
        && !matches!(
            context.checkout.as_deref(),
            Some("read_only" | "pull_request_read_only")
        )
}

/// The model-visible `submit_for_pr` tool.
pub struct SubmitForPrTool {
    callback: SubmitForPrCallback,
    correlation_key: String,
    role: String,
    action: String,
}

impl SubmitForPrTool {
    pub fn new(context: &WorkspaceContext, callback: SubmitForPrCallback) -> Self {
        Self {
            callback,
            correlation_key: context.correlation_key.clone(),
            role: context.work_item.role.clone(),
            action: context.action.clone(),
        }
    }
}

#[async_trait]
impl Tool for SubmitForPrTool {
    fn name(&self) -> &str {
        "submit_for_pr"
    }

    fn label(&self) -> &str {
        "submit_for_pr"
    }

    fn description(&self) -> &str {
        "Submit the current workspace to the host-controlled PR gate. Input: \
         optional { summary: string }. The host returns accepted=false with \
         gate details when more fixes are needed, or accepted=true when you may \
         emit the terminal WorkspaceResult JSON. This tool only relays the \
         request/response; it never pushes or opens a PR."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "summary": {
                    "type": "string",
                    "description": "Optional short note describing what is ready for PR."
                }
            }
        })
    }

    fn effects(&self) -> ToolEffects {
        // Control-plane submit attempts must not be batched in parallel with
        // writes or process tools; the host should see a coherent workspace.
        ToolEffects::process()
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        input: serde_json::Value,
        _on_update: Option<Box<dyn Fn(ToolUpdate) + Send + Sync>>,
    ) -> Result<ToolOutput> {
        let summary = parse_summary(input)?;
        let request = SubmitForPrRequest {
            protocol_version: PROTOCOL_VERSION,
            correlation_key: self.correlation_key.clone(),
            role: self.role.clone(),
            action: self.action.clone(),
            summary,
        };
        let response = (self.callback)(request).await;
        let details = serde_json::to_value(&response).ok();
        Ok(ToolOutput {
            content: vec![ContentBlock::Text(TextContent {
                text: render_response(&response),
                text_signature: None,
            })],
            details,
            // A host rejection is a normal gate result, not a tool execution
            // failure; keep it in-session so the model can edit and resubmit.
            is_error: false,
        })
    }
}

fn parse_summary(input: serde_json::Value) -> Result<Option<String>> {
    if input.is_null() {
        return Ok(None);
    }
    let Some(object) = input.as_object() else {
        return Err(Error::tool(
            "submit_for_pr",
            "submit_for_pr input must be an object",
        ));
    };
    match object.get("summary") {
        Some(value) => value
            .as_str()
            .map(|summary| Some(summary.to_string()))
            .ok_or_else(|| Error::tool("submit_for_pr", "summary must be a string")),
        None => Ok(None),
    }
}

fn render_response(response: &SubmitForPrResponse) -> String {
    let mut text = if response.accepted {
        format!(
            "submit_for_pr accepted by host: {}\nYou may now emit the terminal WorkspaceResult JSON.",
            response.message
        )
    } else {
        format!(
            "submit_for_pr rejected by host: {}\nContinue fixing the workspace, then call submit_for_pr again.",
            response.message
        )
    };
    if !response.gates.is_empty() {
        text.push_str("\n\nHost gate reports:");
        for gate in &response.gates {
            push_gate_report(&mut text, gate);
        }
    }
    text
}

fn push_gate_report(text: &mut String, gate: &SubmitForPrGate) {
    let argv = if gate.argv.is_empty() {
        "(no argv)".to_string()
    } else {
        gate.argv.join(" ")
    };
    text.push_str(&format!(
        "\n- {}: status={} code={} timeout={} elapsed_ms={} cwd={} argv={}",
        gate.command_id,
        gate.exit_status,
        gate.exit_code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "none".to_string()),
        gate.timed_out,
        gate.elapsed_ms,
        gate.cwd,
        argv
    ));
    if !gate.stdout_tail.is_empty() {
        text.push_str(&format!("\n  stdout_tail:\n{}", gate.stdout_tail));
    }
    if !gate.stderr_tail.is_empty() {
        text.push_str(&format!("\n  stderr_tail:\n{}", gate.stderr_tail));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use temper_protocol_agent::{WorkspaceRepository, WorkspaceWorkItem};

    #[test]
    fn availability_requires_writable_engineer_session() {
        let writable_engineer = context("engineer", "writable", "writable");
        assert!(submit_for_pr_available(&writable_engineer));

        let read_only_engineer = context("engineer", "writable", "read_only");
        assert!(!submit_for_pr_available(&read_only_engineer));

        let reviewer = context("reviewer", "writable", "pull_request_read_only");
        assert!(!submit_for_pr_available(&reviewer));

        let architect = context("architect", "writable", "read_only");
        assert!(!submit_for_pr_available(&architect));

        let no_writable_repo = context("engineer", "read_only", "writable");
        assert!(!submit_for_pr_available(&no_writable_repo));
    }

    fn context(role: &str, repo_access: &str, checkout: &str) -> WorkspaceContext {
        WorkspaceContext {
            trace_context: None,
            artifact_context: None,
            repos: vec![WorkspaceRepository {
                id: "r".to_string(),
                owner: "o".to_string(),
                name: "n".to_string(),
                default_branch: "main".to_string(),
                dir: "n".to_string(),
                access: repo_access.to_string(),
                base_branch: "main".to_string(),
                branch_hint: Some("agent/x".to_string()),
            }],
            work_item: WorkspaceWorkItem {
                role: role.to_string(),
                queue: "code".to_string(),
                kind: "code".to_string(),
                target: "Issue { number: 1 }".to_string(),
                context: "{}".to_string(),
            },
            action: "open_pr".to_string(),
            correlation_key: "x".to_string(),
            checkout: Some(checkout.to_string()),
            allowed_verdicts: Vec::new(),
            verdict_contracts: Default::default(),
            source_metadata: Default::default(),
            guidance: Default::default(),
            pull_request_freshness: None,
            agent_session: None,
        }
    }

    #[test]
    fn tool_output_preserves_structured_response_details() {
        let context = context("engineer", "writable", "writable");
        let response = SubmitForPrResponse {
            accepted: false,
            message: "gate failed".to_string(),
            gates: vec![SubmitForPrGate {
                command_id: "cargo-test".to_string(),
                argv: vec!["cargo".to_string(), "test".to_string()],
                cwd: "/ws".to_string(),
                exit_status: "failed".to_string(),
                exit_code: Some(101),
                stdout_tail: "stdout".to_string(),
                stderr_tail: "stderr".to_string(),
                timed_out: false,
                elapsed_ms: 99,
            }],
        };
        let response_for_tool = response.clone();
        let tool = SubmitForPrTool::new(
            &context,
            Arc::new(move |_| {
                let response = response_for_tool.clone();
                Box::pin(async move { response })
            }),
        );
        let output = temper_agent_io::block_on(async move {
            tool.execute(
                "call_submit",
                serde_json::json!({ "summary": "ready" }),
                None,
            )
            .await
        })
        .expect("tool executes");

        assert!(!output.is_error, "host rejection is a normal tool result");
        assert_eq!(
            output.details.expect("structured details"),
            serde_json::to_value(&response).expect("response to value")
        );
        let text = match &output.content[0] {
            ContentBlock::Text(text) => &text.text,
            other => panic!("expected text block, got {other:?}"),
        };
        assert!(text.contains("rejected by host"));
        assert!(text.contains("cargo-test"));
    }

    #[test]
    fn render_response_keeps_rejection_as_actionable_text() {
        let response = SubmitForPrResponse {
            accepted: false,
            message: "tests failed".to_string(),
            gates: vec![SubmitForPrGate {
                command_id: "cargo-test".to_string(),
                argv: vec!["cargo".to_string(), "test".to_string()],
                cwd: "/ws".to_string(),
                exit_status: "failed".to_string(),
                exit_code: Some(101),
                stdout_tail: "running".to_string(),
                stderr_tail: "boom".to_string(),
                timed_out: false,
                elapsed_ms: 42,
            }],
        };
        let rendered = render_response(&response);
        assert!(rendered.contains("rejected by host"));
        assert!(rendered.contains("call submit_for_pr again"));
        assert!(rendered.contains("cargo-test"));
        assert!(rendered.contains("stderr_tail"));
    }
}
