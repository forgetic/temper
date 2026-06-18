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
//! - **Outbound step-progress (agent → worker), stream:** zero or more
//!   [`StepProgress`] records, one per coherent step boundary, emitted on the
//!   agent's **stdout** as line-delimited JSON. Most are crash-recovery
//!   checkpoint markers — *what was done and what was pushed* — emitted
//!   **after** the corresponding commit is pushed. A record may also carry a
//!   plan publication with no pushed commit so the host/orchestrator can publish
//!   checklist-worthy plans without asking the model to perform forge actions.
//! - **Outbound result (agent → worker), terminal:** a [`WorkspaceResult`]
//!   written to the file named by the agent's `--result` flag. The result may
//!   include an optional structured implementation plan; omitting it preserves
//!   the legacy file protocol.
//!
//! # Recovery, not transactions
//!
//! Step-progress gives **resumability**, not exactly-once semantics: a crash
//! between the push and the marker leaves a small inconsistency window, which
//! the next agent reconciles by reading the branch diff. Push at coherent step
//! boundaries; let the marker reflect only what was pushed.

use serde::{Deserialize, Serialize};

mod plan;

pub use plan::{PlanPublication, PlanPublicationTarget};

/// Wire-format version. Bumped on any breaking change to the context, result,
/// or step-progress shapes. The context and each step-progress record embed it
/// so a mismatch is a clean protocol error rather than a silent misparse.
pub const PROTOCOL_VERSION: u32 = 1;

/// The **single** secret env var the agent consumes: the provider credential as
/// a JSON document (see [`ProviderCredentialJson`]).
///
/// The worker reads deployment config + secret sources, builds this JSON, and
/// injects it into the spawned agent's environment. Every non-secret input (the
/// context/result paths, the workspace, the provider/model/url, the
/// deadline/cadence) is a CLI flag — only the credential crosses as env.
pub const PROVIDER_CREDENTIALS_ENV: &str = "TEMPER_AGENT_PROVIDER_CREDENTIALS_JSON";

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
    /// orchestration world, carried here, and stamped by the agent onto every
    /// [`StepProgress`] and onto everything it emits to the out-of-band
    /// control/observability plane. This is the single deliberate bridge
    /// between the two planes.
    pub correlation_key: String,
    /// Checkout mode token: `writable`, `read_only`, or `pull_request_read_only`.
    #[serde(default)]
    pub checkout: Option<String>,
    /// The verdict vocabulary the assigned action declares. Empty means this
    /// action has no verdict branch and should produce a branch/diff result.
    #[serde(default)]
    pub allowed_verdicts: Vec<String>,
    #[serde(default)]
    pub guidance: WorkspaceGuidance,
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
/// Verdict absent ⇒ head path (the working-tree diff is the product). The
/// optional [`WorkspaceResult::plan`] carries structured implementation phases
/// for non-trivial engineer work without overloading the free-text summary; it
/// is omitted by legacy agents and for trivial/no-checklist jobs.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<ImplementationPlan>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review_body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<WorkspaceResultChild>,
}

/// Structured plan data for non-trivial implementation work.
///
/// Phases are ordered, human-readable labels. Zero or one phase means the work
/// is trivial enough that no PR checklist ceremony should be created; two or
/// more phases are checklist-worthy and may be rendered one checkbox per phase
/// by later workflow layers.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ImplementationPlan {
    #[serde(default)]
    pub phases: Vec<String>,
}

impl ImplementationPlan {
    /// Minimum number of phases that should produce PR checklist ceremony.
    pub const CHECKLIST_PHASE_COUNT: usize = 2;

    /// Whether the plan is substantial enough to render as a PR checklist.
    pub fn is_checklist_worthy(&self) -> bool {
        self.phases.len() >= Self::CHECKLIST_PHASE_COUNT
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceResultChild {
    pub slug: String,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Target repository as an `owner/name` path. `None` = the parent's repo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_repo: Option<String>,
}

/// One durable human-facing progress marker emitted by the agent/host at a
/// coherent boundary.
///
/// Most markers are crash-recovery checkpoints emitted *after* the corresponding
/// commit is pushed. Plan-publication markers may carry only the model-authored
/// plan plus host-filled repository routing before any commit exists. The worker
/// relays each record to the forge/orchestrator as durable progress (a ticked
/// checklist item, a PR-body update). Everything high-frequency (token deltas,
/// tool calls) belongs on the control plane, not here.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StepProgress {
    /// Echoes [`WorkspaceContext::correlation_key`] so the worker (and the
    /// control plane) can join this marker to its job without parsing prose.
    pub correlation_key: String,
    /// Monotonic step index within the turn, starting at 1.
    pub step: u32,
    /// Short imperative label of the step, e.g. "write failing test". For a
    /// planned implementation phase, use the phase label exactly (modulo
    /// whitespace) so the daemon can tick the matching PR checklist item.
    pub status: String,
    /// Step lifecycle phase.
    #[serde(default)]
    pub state: StepState,
    /// Commit sha this step pushed, when it pushed one. `None` for read-only or
    /// not-yet-pushed steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pushed_sha: Option<String>,
    /// Optional one-line human note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Optional plan publication for this progress marker. Omitted by legacy
    /// agents and by steps that are not publishing a plan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_publication: Option<PlanPublication>,
}

/// Lifecycle phase of a [`StepProgress`] record.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepState {
    /// The step has begun (no checkpoint yet).
    Started,
    /// The step finished and its work (if any) is pushed — a safe resume point.
    #[default]
    Done,
}

impl StepProgress {
    /// Serializes to a single JSON line (no embedded newline) for the
    /// line-delimited stdout stream.
    pub fn to_line(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Parses one line of the stdout stream into a [`StepProgress`]. Returns
    /// `Ok(None)` for a blank line so the worker can skip framing whitespace.
    pub fn from_line(line: &str) -> Result<Option<Self>, serde_json::Error> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        serde_json::from_str(trimmed).map(Some)
    }
}

#[cfg(test)]
mod plan_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_progress_round_trips_one_line() {
        let progress = StepProgress {
            correlation_key: "pr-for-code-7".to_string(),
            step: 2,
            status: "write failing test".to_string(),
            state: StepState::Done,
            pushed_sha: Some("abc123".to_string()),
            note: None,
            plan_publication: None,
        };
        let line = progress.to_line().expect("serialize");
        assert!(!line.contains('\n'));
        let parsed = StepProgress::from_line(&line)
            .expect("parse")
            .expect("non-empty");
        assert_eq!(parsed, progress);
    }

    #[test]
    fn blank_lines_are_skipped() {
        assert_eq!(StepProgress::from_line("   ").expect("ok"), None);
        assert_eq!(StepProgress::from_line("").expect("ok"), None);
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
    fn step_state_defaults_to_done_when_absent() {
        let parsed: StepProgress =
            serde_json::from_str(r#"{"correlation_key":"k","step":1,"status":"did a thing"}"#)
                .expect("parse without state");
        assert_eq!(parsed.state, StepState::Done);
        assert_eq!(parsed.pushed_sha, None);
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
        assert!(value.get("plan").is_none());
        assert!(value.get("children").is_none());
    }

    #[test]
    fn workspace_result_parses_legacy_result_without_plan() {
        let parsed: WorkspaceResult = serde_json::from_str(
            r#"{"summary":"legacy head path","body":"existing result shape"}"#,
        )
        .expect("legacy result parses");
        assert_eq!(parsed.summary.as_deref(), Some("legacy head path"));
        assert_eq!(parsed.plan, None);
    }

    #[test]
    fn workspace_result_carries_optional_implementation_plan() {
        let result = WorkspaceResult {
            summary: Some("implemented protocol polish".to_string()),
            plan: Some(ImplementationPlan {
                phases: vec![
                    "extend protocol DTO".to_string(),
                    "update agent prompt".to_string(),
                    "cover result parsing".to_string(),
                ],
            }),
            ..Default::default()
        };
        let json = serde_json::to_string(&result).expect("serialize");
        let parsed: WorkspaceResult = serde_json::from_str(&json).expect("parse");
        let plan = parsed.plan.expect("plan present");
        assert_eq!(
            plan.phases,
            vec![
                "extend protocol DTO".to_string(),
                "update agent prompt".to_string(),
                "cover result parsing".to_string(),
            ]
        );
        assert!(plan.is_checklist_worthy());
    }

    #[test]
    fn implementation_plan_checklist_rule_treats_zero_or_one_phase_as_trivial() {
        let empty = ImplementationPlan { phases: Vec::new() };
        assert!(!empty.is_checklist_worthy());

        let one = ImplementationPlan {
            phases: vec!["fix typo".to_string()],
        };
        assert!(!one.is_checklist_worthy());

        let two = ImplementationPlan {
            phases: vec!["add test".to_string(), "implement fix".to_string()],
        };
        assert!(two.is_checklist_worthy());
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
