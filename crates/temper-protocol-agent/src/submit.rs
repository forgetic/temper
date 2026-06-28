//! DTOs for the live host-controlled `submit_for_pr` side channel.

use serde::{Deserialize, Serialize};

/// CLI flag carrying the worker-owned local submit-for-PR side-channel address.
///
/// When present, the out-of-process agent may expose a `submit_for_pr` tool and
/// connect back to this local endpoint for host-gated submit attempts. It is a
/// non-secret, per-run carrier flag (the provider credential remains the only
/// secret environment input).
pub const SUBMIT_FOR_PR_ADDRESS_FLAG: &str = "--submit-for-pr-address";

/// Live request emitted by the agent-side `submit_for_pr` tool and serviced by
/// the host/worker side while the same agent run remains alive.
///
/// The request is intentionally small: the host already owns the prepared
/// workspace root and full `WorkspaceContext` for the run, so the agent only
/// relays the workstream identity plus an optional model-authored note.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SubmitForPrRequest {
    /// Protocol version for this request/response side channel.
    pub protocol_version: u32,
    /// Per-job correlation id, copied from `WorkspaceContext::correlation_key`.
    pub correlation_key: String,
    /// Role that is attempting the submit (normally `engineer`).
    pub role: String,
    /// Workflow action being completed (normally `open_pr`).
    pub action: String,
    /// Optional agent-authored note about what is being submitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// Host response returned to the same live agent run after a `submit_for_pr`
/// request.
///
/// `accepted=false` is a normal tool result, not a terminal agent failure: the
/// model should keep its session context, make more edits, and submit again.
/// `accepted=true` tells the model the host gate is satisfied and it may emit
/// the terminal `WorkspaceResult` JSON. Gate reports are structured so the #518
/// pre-push runner fields can be carried without parsing prose.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SubmitForPrResponse {
    /// Whether the host accepts the workspace for the PR handoff.
    pub accepted: bool,
    /// Human-readable host guidance for the model.
    pub message: String,
    /// Structured command/gate reports, if the host ran any checks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gates: Vec<SubmitForPrGate>,
}

impl SubmitForPrResponse {
    /// A host success response with no command reports.
    pub fn accepted(message: impl Into<String>) -> Self {
        Self {
            accepted: true,
            message: message.into(),
            gates: Vec::new(),
        }
    }

    /// A host failure response with no command reports.
    pub fn rejected(message: impl Into<String>) -> Self {
        Self {
            accepted: false,
            message: message.into(),
            gates: Vec::new(),
        }
    }
}

/// Structured report for one host-side submit gate command.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SubmitForPrGate {
    /// Stable host-assigned command id (for logs/report correlation).
    pub command_id: String,
    /// Command argv exactly as the host describes it.
    #[serde(default)]
    pub argv: Vec<String>,
    /// Working directory used by the command.
    pub cwd: String,
    /// Host-readable exit status (`passed`, `failed`, `timeout`, ...).
    pub exit_status: String,
    /// Process exit code when one exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Tail of stdout captured by the host.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stdout_tail: String,
    /// Tail of stderr captured by the host.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stderr_tail: String,
    /// Whether the host timed the command out.
    #[serde(default)]
    pub timed_out: bool,
    /// Elapsed wall-clock time in milliseconds.
    pub elapsed_ms: u64,
}
