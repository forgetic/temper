//! The worker ↔ agent process protocol (plane 1).
//!
//! This crate is the **contract** between the orchestration worker
//! (`smith-worker`) and an out-of-process coding agent. Smith owns the
//! protocol; agents implement it (the reference implementation is
//! `anvil-agent` in the sibling `anvil` repo). It is serde-only and depends
//! on nothing else, so a third-party agent can speak it without pulling in
//! smith, and the worker can drive any agent without linking agent/LLM code.
//!
//! # Shape
//!
//! The exchange is deliberately narrow:
//!
//! - **Inbound (worker → agent), one-shot:** a [`WorkspaceContext`] — the
//!   repository, role, assigned action, branch, verdict vocabulary, and work item the worker
//!   assembled, carrying the [`WorkspaceContext::correlation_key`] that is the
//!   *only* bridge to the out-of-band control/observability plane. The worker
//!   writes it to a file and passes its path as the agent's `--context` flag,
//!   running the agent in the prepared checkout (`--workspace`, also cwd).
//! - **Live side channels (agent ↔ worker):** writable engineer agents may call
//!   `submit_for_pr`, and every role may use bounded read-only Forge context
//!   operations when the worker configured a fetch host. The worker services
//!   per-run local request/response channels; Forge credentials and assignment
//!   identity never enter the child protocol.
//! - **Outbound result (agent → worker), terminal:** a [`WorkspaceResult`]
//!   written to the file named by the agent's `--result` flag.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use temper_protocol_context::W3cTraceContext;
pub use temper_protocol_context::{
    ARTIFACT_CONTEXT_VERSION, ArtifactContextBundle, ArtifactContextDiagnostic,
    ArtifactContextDiagnosticCode, ArtifactContextTruncation, ArtifactIndexEntry,
    ArtifactReference, ArtifactRelation, ArtifactRelationType, ArtifactRepository,
    ArtifactSnapshot, ArtifactSummary, ArtifactType, ArtifactWorkflowContext,
    ForgeContextErrorCode, ForgeContextOperation, ForgeContextResult, ForgeGetItemOperation,
    ForgeGetItemResult, ForgeItemComment, ForgeListRelatedOperation, ForgeListRelatedResult,
    ForgeRelatedEdge, ForgeRelationType, WorkflowArtifactReference, WorkflowChildIdentity,
};
use temper_verdict::{VerdictChildView, VerdictContracts, VerdictResultView};

mod forge;
pub use forge::{
    FORGE_CONTEXT_ADDRESS_FLAG, ForgeContextRequest, ForgeContextResponse, ForgeContextToolOutcome,
};
mod containment;
pub use containment::*;
mod lifecycle;
pub use lifecycle::{
    AGENT_LIFECYCLE_ADDRESS_FLAG, AGENT_LIFECYCLE_PROTOCOL_VERSION, AgentCancellationStage,
    AgentLifecycleAgentStatusV1, AgentLifecycleCancellationAckV1,
    AgentLifecycleCancellationAcknowledgementV1, AgentLifecycleCommandV1, AgentLifecycleEventV1,
    AgentLifecycleFrameV1, AgentLifecycleHelloV1, AgentLifecycleModelStatusV1,
    AgentLifecycleScopeV1, AgentLifecycleToolStatusV1, AgentLifecycleValidationError,
    MAX_AGENT_LIFECYCLE_CANCEL_REASON_BYTES, MAX_AGENT_LIFECYCLE_FRAME_BYTES,
    MAX_AGENT_LIFECYCLE_ID_BYTES, MAX_AGENT_LIFECYCLE_TOOL_NAME_BYTES,
};
mod submit;
pub use submit::{
    SUBMIT_FOR_PR_ADDRESS_FLAG, SubmitForPrGate, SubmitForPrRequest, SubmitForPrResponse,
};

/// Wire-format version. Bumped on any breaking change to the context, result,
/// or provider-credential shapes. The context embeds it so a mismatch is a clean
/// protocol error rather than a silent misparse.
pub const PROTOCOL_VERSION: u32 = 1;

/// The **single** secret env var the agent consumes: the provider credential as
/// a JSON document (see [`ProviderCredentialJson`]).
///
/// The worker reads deployment config + secret sources, builds this JSON, and
/// injects it into the spawned agent's environment. Every non-secret input (the
/// context/result paths, the workspace, the provider/model/url, the
/// workspace path) is a CLI flag — only the credential crosses as env.
pub const PROVIDER_CREDENTIALS_ENV: &str = "TEMPER_AGENT_PROVIDER_CREDENTIALS_JSON";

/// Process-boundary flag naming the resolved first-party operation-limit file.
pub const RUNTIME_LIMITS_FLAG: &str = "--runtime-limits";

/// Complete, non-secret first-party agent operation limits.
///
/// This DTO deliberately stores seconds rather than runtime clock types so the
/// worker/agent protocol crate remains serde-only. Runtime tiers convert these
/// values to monotonic durations at their boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentRuntimeLimitsV1 {
    pub tool_timeout_secs: u64,
    pub model_connect_timeout_secs: u64,
    pub model_idle_timeout_secs: u64,
}

impl Default for AgentRuntimeLimitsV1 {
    fn default() -> Self {
        Self {
            tool_timeout_secs: 600,
            model_connect_timeout_secs: 120,
            model_idle_timeout_secs: 120,
        }
    }
}

impl AgentRuntimeLimitsV1 {
    pub fn validate(self) -> Result<Self, String> {
        for (field, value) in [
            ("tool_timeout_secs", self.tool_timeout_secs),
            (
                "model_connect_timeout_secs",
                self.model_connect_timeout_secs,
            ),
            ("model_idle_timeout_secs", self.model_idle_timeout_secs),
        ] {
            if value == 0 {
                return Err(format!("{field} must be greater than zero"));
            }
        }
        Ok(self)
    }

    pub fn from_json(raw: &str) -> Result<Self, String> {
        serde_json::from_str::<Self>(raw.trim())
            .map_err(|error| error.to_string())?
            .validate()
    }

    pub fn to_json(self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(&self)
    }
}

/// Process-boundary flag naming a non-secret JSON tool configuration file.
///
/// The worker writes this only when an agent-local toolset is enabled for the
/// current workflow role. The reference agent parses these settings and uses
/// them to register safe codebase-memory MCP tools in coding-agent runs.
pub const TOOL_CONFIG_FLAG: &str = "--tool-config";

/// The non-secret agent tool configuration file the worker may hand the agent.
///
/// The top-level object is intentionally small and future-friendly:
///
/// ```json
/// {"codebase_memory":{"mode":"auto","command":"codebase-memory-mcp","args":[],"roles":["*"],"index":"background","startup_timeout_secs":5,"index_timeout_secs":30}}
/// ```
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct AgentToolConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codebase_memory: Option<CodebaseMemoryToolConfig>,
}

impl AgentToolConfig {
    /// Returns true when any configured tool applies to `role`.
    pub fn enabled_for_role(&self, role: &str) -> bool {
        self.codebase_memory
            .as_ref()
            .is_some_and(|config| config.applies_to_role(role))
    }

    /// Parses and validates a tool config JSON document.
    pub fn from_json(raw: &str) -> Result<Self, String> {
        let config: Self = serde_json::from_str(raw.trim()).map_err(|error| error.to_string())?;
        config.validate()?;
        Ok(config)
    }

    /// Serializes the tool config to JSON.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    fn validate(&self) -> Result<(), String> {
        if let Some(config) = &self.codebase_memory {
            config.validate()?;
        }
        Ok(())
    }
}

/// Resolved codebase-memory MCP tool settings.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CodebaseMemoryToolConfig {
    pub mode: CodebaseMemoryMode,
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<String>,
    pub index: CodebaseMemoryIndex,
    pub startup_timeout_secs: u64,
    pub index_timeout_secs: u64,
}

impl CodebaseMemoryToolConfig {
    pub fn applies_to_role(&self, role: &str) -> bool {
        self.roles
            .iter()
            .any(|allowed| allowed == "*" || allowed == role)
    }

    fn validate(&self) -> Result<(), String> {
        if self.command.trim().is_empty() {
            return Err("codebase_memory.command must not be empty".to_string());
        }
        if self.startup_timeout_secs == 0 {
            return Err(
                "codebase_memory.startup_timeout_secs must be greater than zero".to_string(),
            );
        }
        if self.index_timeout_secs == 0 {
            return Err("codebase_memory.index_timeout_secs must be greater than zero".to_string());
        }
        for role in &self.roles {
            if role.trim().is_empty() {
                return Err("codebase_memory.roles entries must not be empty".to_string());
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CodebaseMemoryMode {
    Auto,
    Required,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CodebaseMemoryIndex {
    Off,
    Background,
    Blocking,
}

/// The provider credential the worker hands the agent via
/// [`PROVIDER_CREDENTIALS_ENV`].
///
/// Two shapes, tagged by `type`:
///
/// ```json
/// {"type":"api-key","api_key":"..."}
/// ```
/// ```json
/// {"type":"oauth","access_token":"...","refresh_token":"...","expires_at_unix_seconds":1781701200}
/// ```
///
/// This is the serde-only wire shape; the token bytes are plain `String`s here.
/// The agent immediately re-wraps them in a redacting secret type after parsing,
/// and the worker builds this struct from its own secret sources. Errors that
/// reference a credential must carry only its `type`, never token bytes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ProviderCredentialJson {
    /// A static API key (the DeepSeek / OpenAI-compatible path).
    ApiKey {
        /// The provider API key.
        api_key: String,
    },
    /// OAuth tokens (the ChatGPT/Anthropic subscription path). The agent
    /// materializes these into a pi-format `auth.json` its OAuth loader reads
    /// (and refreshes) in place.
    Oauth {
        /// The current OAuth access token (the per-request bearer).
        access_token: String,
        /// The refresh token, when the worker has one to forward.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        refresh_token: Option<String>,
        /// Access-token expiry as a unix-seconds timestamp.
        expires_at_unix_seconds: i64,
    },
}

impl ProviderCredentialJson {
    /// Parses a credential from its JSON document.
    pub fn from_json(raw: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(raw.trim())
    }

    /// Serializes the credential to its JSON document (the worker→agent env
    /// value). Errors only on a serializer fault, which cannot happen for this
    /// shape.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

/// Env var naming the file the worker wrote the [`WorkspaceContext`] JSON to.
///
/// Legacy channel: the current worker passes the context path as the
/// `--context` CLI flag (and the result path as `--result`), so the in-tree
/// agent no longer reads these. The names are retained for external coders that
/// still speak the old file-via-env protocol.
pub const CONTEXT_ENV: &str = "TEMPER_CODING_WORKSPACE_CONTEXT";
/// Env var naming the file the agent must write its [`WorkspaceResult`] JSON to.
/// Legacy; the worker now passes the path as the `--result` CLI flag.
pub const RESULT_ENV: &str = "TEMPER_CODING_WORKSPACE_RESULT";

/// Durable per-workstream agent-session state that the worker can persist while
/// an implementation PR waits for CI/review/landing, then pass back to a later
/// PR-feedback run for the same `(role, coordination_key)`.
///
/// The protocol keeps this deliberately small and provider-agnostic. `session_id`
/// is the stable host-visible id (also useful as a provider session header when
/// supported); `state` is an optional extension bag for future agents that need
/// more than an id. Agents that do not understand session state can ignore it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AgentSessionState {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<serde_json::Value>,
}

impl AgentSessionState {
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            state: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PullRequestFreshness {
    pub repository_id: String,
    pub repo: String,
    pub role: String,
    pub queue: String,
    pub action: String,
    pub number: u64,
    pub pull_request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_condition: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub queue_labels: Vec<String>,
}

/// The work-item context the worker hands the agent for one turn.
///
/// Moved here from the agent's coding-loop crate so it is owned by the wire
/// contract; the agent re-exports it. Serde shape is unchanged.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceContext {
    /// Optional assignment-delivery context. Separate workstream runs do not
    /// retain it as a long-lived parent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_context: Option<W3cTraceContext>,
    /// The repositories assembled into this workspace, laid out as siblings
    /// under the agent's working directory (ADR 0023). The first is the primary
    /// — home of the coordinating artifact. For a plain single-repo job this is
    /// a one-element list.
    pub repos: Vec<WorkspaceRepository>,
    pub work_item: WorkspaceWorkItem,
    /// Versioned graph context for the coordinating artifact and related work.
    /// The singular `work_item.context` remains unchanged for compatibility.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_context: Option<ArtifactContextBundle>,
    /// Workflow action/transition this workspace turn is assigned to perform.
    pub action: String,
    /// Per-job correlation id (the coordination key). Minted in the
    /// orchestration world and carried here so logs and terminal results can be
    /// joined to the assigned work without parsing prose.
    pub correlation_key: String,
    /// Checkout mode token: `writable`, `read_only`, `pull_request_read_only`,
    /// or `pull_request_writable`.
    #[serde(default)]
    pub checkout: Option<String>,
    /// The verdict vocabulary the assigned action declares. Empty means this
    /// action has no verdict branch and should produce a branch/diff result.
    #[serde(default)]
    pub allowed_verdicts: Vec<String>,
    /// Workflow-derived terminal product requirements keyed by verdict. Empty
    /// keeps contexts from older workers backward compatible.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub verdict_contracts: VerdictContracts,
    /// Assignment-time source metadata used for pre-mutation product checks.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub source_metadata: BTreeMap<String, String>,
    #[serde(default)]
    pub guidance: WorkspaceGuidance,
    /// Freshness guard facts for PR-head writable jobs. The worker revalidates
    /// these before the final push; absent for ordinary jobs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_request_freshness: Option<PullRequestFreshness>,
    /// Persisted agent-session state for this workstream, when the worker has a
    /// saved or newly-created session to attach. This is host control-plane
    /// state, not prompt context; the reference agent consumes it for provider
    /// session identity and does not render it into the work-item prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session: Option<AgentSessionState>,
}

impl WorkspaceContext {
    /// The primary repository (home of the coordinating artifact).
    pub fn primary(&self) -> Option<&WorkspaceRepository> {
        self.repos.first()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceRepository {
    pub id: String,
    pub owner: String,
    pub name: String,
    pub default_branch: String,
    /// Directory under the workspace root this repo is checked out into, chosen
    /// so inter-repo path dependencies resolve (e.g. `temper`, `smith`, `skein`
    /// as flat siblings).
    pub dir: String,
    /// Repository-manifest policy: `writable` makes the repository eligible for
    /// edits when the effective checkout mode also permits mutation;
    /// `read_only` means it is present only for inspection/build resolution and
    /// is never pushed. A read-only checkout mode overrides this policy.
    pub access: String,
    pub base_branch: String,
    /// Work branch a writable repo's diff is pushed to. Absent for read-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_hint: Option<String>,
}

impl WorkspaceRepository {
    pub fn is_writable(&self) -> bool {
        self.access == "writable"
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceWorkItem {
    pub role: String,
    pub queue: String,
    pub kind: String,
    /// Debug-formatted target, e.g. `Issue { number: ItemNumber(7) }`.
    pub target: String,
    /// Inner work-item JSON string (artifact title/body/labels).
    pub context: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceGuidance {
    #[serde(default)]
    pub role_guidance: Option<String>,
    #[serde(default)]
    pub tool_guidance: Option<String>,
    #[serde(default)]
    pub tool_constraints: Vec<String>,
}

/// The agent's terminal work product for one turn.
///
/// Verdict absent ⇒ head path (the working-tree diff is the product).
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// No-verdict engineer success: implementation PR title. Verdict results
    /// may also use this as the authored title for routed transitions whose
    /// `create_pull_request` effect declares a PR artifact kind.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// No-verdict engineer success: implementation PR report body. Verdict
    /// results preserve the legacy meaning: routed issue/review body.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review_body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<WorkspaceResultChild>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceResultChild {
    pub slug: String,
    pub title: String,
    pub body: String,
    /// Workflow artifact kind for this child issue. Omitted defaults to `code`
    /// on the daemon verdict fan-out path for backward compatibility.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Target repository as an `owner/name` path. `None` = the parent's repo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_repo: Option<String>,
}

impl VerdictResultView for WorkspaceResult {
    type Child = WorkspaceResultChild;

    fn verdict(&self) -> Option<&str> {
        self.verdict.as_deref()
    }

    fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    fn body(&self) -> Option<&str> {
        self.body.as_deref().or(self.review_body.as_deref())
    }

    fn children(&self) -> &[Self::Child] {
        &self.children
    }
}

impl VerdictChildView for WorkspaceResultChild {
    fn slug(&self) -> &str {
        &self.slug
    }

    fn title(&self) -> &str {
        &self.title
    }

    fn body(&self) -> &str {
        &self.body
    }

    fn kind(&self) -> Option<&str> {
        self.kind.as_deref()
    }

    fn depends_on(&self) -> &[String] {
        &self.depends_on
    }
}

#[cfg(test)]
mod tests;
