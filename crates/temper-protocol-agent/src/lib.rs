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
//! - **Live side channel (agent ↔ worker):** writable engineer agents may call
//!   `submit_for_pr` during the run. The worker services a local request/response
//!   channel and returns a [`SubmitForPrResponse`] to the same live agent session
//!   so failed gates can be fixed and retried before the terminal result.
//! - **Outbound result (agent → worker), terminal:** a [`WorkspaceResult`]
//!   written to the file named by the agent's `--result` flag.

use serde::{Deserialize, Serialize};

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
    /// The repositories assembled into this workspace, laid out as siblings
    /// under the agent's working directory (ADR 0023). The first is the primary
    /// — home of the coordinating artifact. For a plain single-repo job this is
    /// a one-element list.
    pub repos: Vec<WorkspaceRepository>,
    pub work_item: WorkspaceWorkItem,
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
    /// `writable` (the agent may edit it; a diff opens a PR) or `read_only`
    /// (present only so the build resolves; never pushed).
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
    /// ignore this field and continue to use `body` for routed content.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codebase_memory_tool_config_round_trips_and_filters_roles() {
        let json = r#"{
            "codebase_memory": {
                "mode": "auto",
                "command": "codebase-memory-mcp",
                "args": ["--cache", "local"],
                "roles": ["engineer"],
                "index": "background",
                "startup_timeout_secs": 5,
                "index_timeout_secs": 30
            }
        }"#;
        let config = AgentToolConfig::from_json(json).expect("parse tool config");
        assert!(config.enabled_for_role("engineer"));
        assert!(!config.enabled_for_role("architect"));
        let rendered = config.to_json().expect("serialize tool config");
        assert_eq!(AgentToolConfig::from_json(&rendered).unwrap(), config);
    }

    #[test]
    fn codebase_memory_tool_config_rejects_invalid_values() {
        for json in [
            r#"{"codebase_memory":{"mode":"auto","command":"","roles":["*"],"index":"background","startup_timeout_secs":5,"index_timeout_secs":30}}"#,
            r#"{"codebase_memory":{"mode":"auto","command":"cmd","roles":[""],"index":"background","startup_timeout_secs":5,"index_timeout_secs":30}}"#,
            r#"{"codebase_memory":{"mode":"auto","command":"cmd","roles":["*"],"index":"background","startup_timeout_secs":0,"index_timeout_secs":30}}"#,
            r#"{"codebase_memory":{"mode":"auto","command":"cmd","roles":["*"],"index":"eventually","startup_timeout_secs":5,"index_timeout_secs":30}}"#,
        ] {
            assert!(
                AgentToolConfig::from_json(json).is_err(),
                "invalid config should fail: {json}"
            );
        }
    }

    #[test]
    fn api_key_credential_round_trips() {
        let credential =
            ProviderCredentialJson::from_json(r#"{"type":"api-key","api_key":"sk-x"}"#)
                .expect("parse api-key");
        assert_eq!(
            credential,
            ProviderCredentialJson::ApiKey {
                api_key: "sk-x".to_string(),
            }
        );
        let json = credential.to_json().expect("serialize");
        assert_eq!(
            ProviderCredentialJson::from_json(&json).expect("re-parse"),
            credential
        );
    }

    #[test]
    fn oauth_credential_parses_with_and_without_refresh() {
        let with_refresh = ProviderCredentialJson::from_json(
            r#"{"type":"oauth","access_token":"a","refresh_token":"r","expires_at_unix_seconds":1781701200}"#,
        )
        .expect("parse oauth");
        assert_eq!(
            with_refresh,
            ProviderCredentialJson::Oauth {
                access_token: "a".to_string(),
                refresh_token: Some("r".to_string()),
                expires_at_unix_seconds: 1_781_701_200,
            }
        );
        let without_refresh = ProviderCredentialJson::from_json(
            r#"{"type":"oauth","access_token":"a","expires_at_unix_seconds":0}"#,
        )
        .expect("parse oauth without refresh");
        assert!(matches!(
            without_refresh,
            ProviderCredentialJson::Oauth {
                refresh_token: None,
                ..
            }
        ));
    }

    #[test]
    fn submit_for_pr_response_carries_structured_gate_data() {
        let response = SubmitForPrResponse {
            accepted: false,
            message: "cargo test failed".to_string(),
            gates: vec![SubmitForPrGate {
                command_id: "pre-push:cargo-test".to_string(),
                argv: vec!["cargo".to_string(), "test".to_string()],
                cwd: "/workspace/temper".to_string(),
                exit_status: "failed".to_string(),
                exit_code: Some(101),
                stdout_tail: "running 1 test".to_string(),
                stderr_tail: "test failed".to_string(),
                timed_out: false,
                elapsed_ms: 1_234,
            }],
        };

        let json = serde_json::to_value(&response).expect("serialize submit response");
        assert_eq!(json["accepted"], false);
        assert_eq!(json["gates"][0]["command_id"], "pre-push:cargo-test");
        assert_eq!(json["gates"][0]["argv"][1], "test");
        assert_eq!(json["gates"][0]["cwd"], "/workspace/temper");
        assert_eq!(json["gates"][0]["exit_status"], "failed");
        assert_eq!(json["gates"][0]["exit_code"], 101);
        assert_eq!(json["gates"][0]["stdout_tail"], "running 1 test");
        assert_eq!(json["gates"][0]["stderr_tail"], "test failed");
        assert_eq!(json["gates"][0]["timed_out"], false);
        assert_eq!(json["gates"][0]["elapsed_ms"], 1_234);
        let round_trip: SubmitForPrResponse = serde_json::from_value(json).expect("round trip");
        assert_eq!(round_trip, response);
    }

    #[test]
    fn workspace_result_omits_empty_optionals_on_the_wire() {
        let result = WorkspaceResult {
            summary: Some("did the thing".to_string()),
            ..Default::default()
        };
        let value = serde_json::to_value(&result).expect("serialize");
        assert_eq!(value["summary"], "did the thing");
        assert!(value.get("verdict").is_none());
        assert!(value.get("children").is_none());
    }

    #[test]
    fn workspace_result_carries_engineer_pr_title_and_body() {
        let result = WorkspaceResult {
            title: Some("Implement durable handoff".to_string()),
            body: Some("# Implementation report\n\nDone.".to_string()),
            summary: Some("implemented handoff".to_string()),
            ..Default::default()
        };
        let value = serde_json::to_value(&result).expect("serialize");
        assert_eq!(value["title"], "Implement durable handoff");
        assert_eq!(value["body"], "# Implementation report\n\nDone.");
        assert_eq!(value["summary"], "implemented handoff");
        assert!(value.get("verdict").is_none());
    }

    #[test]
    fn workspace_result_ignores_legacy_plan_field() {
        let parsed: WorkspaceResult = serde_json::from_str(
            r#"{"summary":"legacy head path","plan":{"phases":["old checklist"]}}"#,
        )
        .expect("legacy result with plan parses");
        assert_eq!(parsed.summary.as_deref(), Some("legacy head path"));
    }

    #[test]
    fn workspace_context_correlation_key_is_required_and_round_trips() {
        let json = r#"{
            "repos": [{"id":"1","owner":"acme","name":"svc","default_branch":"main",
                       "dir":"svc","access":"writable","base_branch":"main",
                       "branch_hint":"smith/engineer/issue-7"}],
            "work_item": {"role":"engineer","queue":"code","kind":"issue","target":"Issue { number: 7 }","context":"{}"},
            "action": "open_pr",
            "correlation_key": "pr-for-code-7"
        }"#;
        let context: WorkspaceContext = serde_json::from_str(json).expect("parse");
        assert_eq!(context.action, "open_pr");
        assert_eq!(context.correlation_key, "pr-for-code-7");
        assert_eq!(context.allowed_verdicts, Vec::<String>::new());
        assert_eq!(context.checkout, None);
        let primary = context.primary().expect("primary repo present");
        assert_eq!(primary.dir, "svc");
        assert!(primary.is_writable());
        assert_eq!(primary.base_branch, "main");
    }

    #[test]
    fn workspace_context_carries_multiple_repos_with_access() {
        let json = r#"{
            "repos": [
                {"id":"1","owner":"ai","name":"temper","default_branch":"main",
                 "dir":"temper","access":"writable","base_branch":"main",
                 "branch_hint":"agent/coord-for-code-42"},
                {"id":"2","owner":"ai","name":"skein","default_branch":"main",
                 "dir":"skein","access":"read_only","base_branch":"main"}
            ],
            "work_item": {"role":"engineer","queue":"code","kind":"issue","target":"Issue { number: 42 }","context":"{}"},
            "action": "open_pr",
            "correlation_key": "coord-for-code-42"
        }"#;
        let context: WorkspaceContext = serde_json::from_str(json).expect("parse");
        assert_eq!(context.action, "open_pr");
        assert_eq!(context.repos.len(), 2);
        assert!(context.repos[0].is_writable());
        assert!(!context.repos[1].is_writable());
        assert_eq!(context.repos[1].branch_hint, None);
    }
}
