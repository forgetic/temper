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
//!   repository, role, branch, verdict vocabulary, and work item the worker
//!   assembled, carrying the [`WorkspaceContext::correlation_key`] that is the
//!   *only* bridge to the out-of-band control/observability plane. The worker
//!   writes it to a file named by [`CONTEXT_ENV`] and runs the agent in the
//!   prepared checkout (cwd).
//! - **Outbound step-progress (agent → worker), stream:** zero or more
//!   [`StepProgress`] records, one per coherent step boundary, emitted on the
//!   agent's **stdout** as line-delimited JSON. Each is a crash-recovery
//!   checkpoint marker — *what was done and what was pushed* — that the worker
//!   relays to the forge (tick a todo, update the PR body). Emitted **after**
//!   the corresponding commit is pushed, so the marker never claims more than
//!   the branch actually holds.
//! - **Outbound result (agent → worker), terminal:** a [`WorkspaceResult`]
//!   written to the file named by [`RESULT_ENV`]. Unchanged from the legacy
//!   file protocol, so existing external coders stay compatible.
//!
//! # Recovery, not transactions
//!
//! Step-progress gives **resumability**, not exactly-once semantics: a crash
//! between the push and the marker leaves a small inconsistency window, which
//! the next agent reconciles by reading the branch diff. Push at coherent step
//! boundaries; let the marker reflect only what was pushed.

use serde::{Deserialize, Serialize};

/// Wire-format version. Bumped on any breaking change to the context, result,
/// or step-progress shapes. The context and each step-progress record embed it
/// so a mismatch is a clean protocol error rather than a silent misparse.
pub const PROTOCOL_VERSION: u32 = 1;

/// Env var naming the file the worker wrote the [`WorkspaceContext`] JSON to.
///
/// Kept byte-for-byte as the legacy `TEMPER_CODING_WORKSPACE_*` names so an
/// existing external coder (e.g. the examples' deterministic `greeting`
/// stand-in) needs no changes to keep working.
pub const CONTEXT_ENV: &str = "TEMPER_CODING_WORKSPACE_CONTEXT";
/// Env var naming the file the agent must write its [`WorkspaceResult`] JSON to.
pub const RESULT_ENV: &str = "TEMPER_CODING_WORKSPACE_RESULT";

/// The work-item context the worker hands the agent for one turn.
///
/// Moved here from the agent's coding-loop crate so it is owned by the wire
/// contract; the agent re-exports it. Serde shape is unchanged.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceContext {
    pub repository: WorkspaceRepository,
    pub work_item: WorkspaceWorkItem,
    pub base_branch: String,
    pub branch_hint: String,
    /// Per-job correlation id. Minted in the orchestration world, carried here,
    /// and stamped by the agent onto every [`StepProgress`] and onto everything
    /// it emits to the out-of-band control/observability plane. This is the
    /// single deliberate bridge between the two planes.
    pub correlation_key: String,
    /// Checkout mode token: `writable`, `read_only`, or `pull_request_read_only`.
    #[serde(default)]
    pub checkout: Option<String>,
    /// The verdict vocabulary the bound action declares. Empty ⇒ the agent
    /// falls back to its built-in per-role verdict menu.
    #[serde(default)]
    pub allowed_verdicts: Vec<String>,
    #[serde(default)]
    pub guidance: WorkspaceGuidance,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceRepository {
    pub id: String,
    pub owner: String,
    pub name: String,
    pub default_branch: String,
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
/// Verdict absent ⇒ head path (the working-tree diff is the product). Serde
/// shape is unchanged from the legacy `WorkspaceResult`.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
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
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Target repository as an `owner/name` path. `None` = the parent's repo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_repo: Option<String>,
}

/// One crash-recovery checkpoint marker, emitted by the agent at a coherent step
/// boundary *after* the corresponding commit is pushed.
///
/// The worker relays each record to the forge as durable, human-facing progress
/// (a ticked checklist item, a PR-body update). It carries only what a human
/// reading the PR wants plus the pushed sha for recovery — everything
/// high-frequency (token deltas, tool calls) belongs on the control plane, not
/// here.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StepProgress {
    /// Echoes [`WorkspaceContext::correlation_key`] so the worker (and the
    /// control plane) can join this marker to its job without parsing prose.
    pub correlation_key: String,
    /// Monotonic step index within the turn, starting at 1.
    pub step: u32,
    /// Short imperative label of the step, e.g. "write failing test". Suitable
    /// as a checklist line on the PR.
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
        assert!(value.get("children").is_none());
    }

    #[test]
    fn workspace_context_correlation_key_is_required_and_round_trips() {
        let json = r#"{
            "repository": {"id":"1","owner":"acme","name":"svc","default_branch":"main"},
            "work_item": {"role":"engineer","queue":"code","kind":"issue","target":"Issue { number: 7 }","context":"{}"},
            "base_branch": "main",
            "branch_hint": "smith/engineer/issue-7",
            "correlation_key": "pr-for-code-7"
        }"#;
        let context: WorkspaceContext = serde_json::from_str(json).expect("parse");
        assert_eq!(context.correlation_key, "pr-for-code-7");
        assert_eq!(context.allowed_verdicts, Vec::<String>::new());
        assert_eq!(context.checkout, None);
    }
}
