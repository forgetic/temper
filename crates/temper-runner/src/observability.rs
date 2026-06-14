//! Provider-neutral observability identity and structured event helpers.
//!
//! This module only formats context and log records. It owns no telemetry
//! backend, reads no provider secrets, and grants no workflow authority.

use std::collections::BTreeMap;

mod events;
pub use events::{
    ActionDispatchEvent, MechanicalReconciliationEvent, RoleDecisionReplyEvent,
    RoleDecisionRequestEvent, ScanSummaryEvent, TransitionExecutionEvent, WorkItemSelectedEvent,
    execution_error_diagnostic_classes, execution_error_failure_class,
    postcondition_outcome_for_error, render_action_dispatch_event,
    render_mechanical_reconciliation_event, render_role_decision_reply_event,
    render_role_decision_request_event, render_scan_summary_event,
    render_transition_execution_event, render_work_item_selected_event, workflow_effect_summary,
};

use serde_json::Value;
use temper_forge::{ItemNumber, RepositoryId};
use temper_workflow::{ArtifactKindId, ArtifactSource, QueueId, RoleId};

/// Marker used when text looks like it may contain a credential or token.
pub const REDACTED: &str = "<redacted>";

/// Artifact target type used in observability identities.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObservabilityArtifactType {
    /// Forge issue artifact.
    Issue,
    /// Forge pull request artifact.
    PullRequest,
}

impl ObservabilityArtifactType {
    /// Stable provider-neutral string form.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Issue => "issue",
            Self::PullRequest => "pull_request",
        }
    }

    fn from_source(source: ArtifactSource) -> Self {
        match source {
            ArtifactSource::Issue { .. } => Self::Issue,
            ArtifactSource::PullRequest { .. } => Self::PullRequest,
        }
    }
}

/// Stable identity for one workflow work item or role-decision request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkItemIdentity {
    /// Deterministic id derived only from provider-neutral coordinates.
    pub work_item_id: String,
    /// Deterministic decision id for correlating role responders with Temper.
    pub decision_id: String,
    /// Optional tick id when a caller has one available.
    pub tick_id: Option<String>,
    /// Backend-neutral repository id used by the Forge trait.
    pub repo: RepositoryId,
    /// Workflow role handling the item.
    pub role: RoleId,
    /// Queue that selected the item.
    pub queue: QueueId,
    /// Forge artifact type.
    pub artifact_type: ObservabilityArtifactType,
    /// Repository-local artifact number.
    pub artifact_number: ItemNumber,
    /// Workflow artifact kind resolved by classification.
    pub artifact_kind: ArtifactKindId,
}

impl WorkItemIdentity {
    /// Builds a deterministic identity from workflow and Forge coordinates.
    pub fn new(
        repo: &RepositoryId,
        role: &RoleId,
        queue: &QueueId,
        target: ArtifactSource,
        artifact_kind: &ArtifactKindId,
    ) -> Self {
        let artifact_type = ObservabilityArtifactType::from_source(target);
        let artifact_number = artifact_number(target);
        let work_item_id = work_item_id(
            repo.as_str(),
            role.as_str(),
            queue.as_str(),
            artifact_type,
            artifact_number,
            artifact_kind.as_str(),
        );
        let decision_id = format!("decision/{work_item_id}");
        Self {
            work_item_id,
            decision_id,
            tick_id: None,
            repo: repo.clone(),
            role: role.clone(),
            queue: queue.clone(),
            artifact_type,
            artifact_number,
            artifact_kind: artifact_kind.clone(),
        }
    }

    /// Returns the same identity associated with a caller-supplied tick id.
    pub fn with_tick_id(mut self, tick_id: impl Into<String>) -> Self {
        let tick_id = tick_id.into();
        self.decision_id = format!(
            "decision/{}/{}",
            length_prefixed("tick", &tick_id),
            self.work_item_id
        );
        self.tick_id = Some(tick_id);
        self
    }

    /// Serializes the identity as authority-neutral JSON context.
    pub fn to_json(&self) -> Value {
        let mut object = serde_json::Map::new();
        object.insert(
            "work_item_id".to_string(),
            Value::String(self.work_item_id.clone()),
        );
        object.insert(
            "decision_id".to_string(),
            Value::String(self.decision_id.clone()),
        );
        if let Some(tick_id) = &self.tick_id {
            object.insert("tick_id".to_string(), Value::String(tick_id.clone()));
        }
        object.insert("repo".to_string(), Value::String(self.repo.to_string()));
        object.insert("role".to_string(), Value::String(self.role.to_string()));
        object.insert("queue".to_string(), Value::String(self.queue.to_string()));
        object.insert(
            "artifact_type".to_string(),
            Value::String(self.artifact_type.as_str().to_string()),
        );
        object.insert(
            "artifact_number".to_string(),
            Value::Number(self.artifact_number.get().into()),
        );
        object.insert(
            "artifact_kind".to_string(),
            Value::String(self.artifact_kind.to_string()),
        );
        Value::Object(object)
    }
}

fn artifact_number(source: ArtifactSource) -> ItemNumber {
    match source {
        ArtifactSource::Issue { number } | ArtifactSource::PullRequest { number } => number,
    }
}

fn work_item_id(
    repo: &str,
    role: &str,
    queue: &str,
    artifact_type: ObservabilityArtifactType,
    artifact_number: ItemNumber,
    artifact_kind: &str,
) -> String {
    format!(
        "work-item/{}/{}/kind:{}/queue:{}/role:{}",
        length_prefixed("repo", repo),
        artifact_segment(artifact_type, artifact_number),
        length_value(artifact_kind),
        length_value(queue),
        length_value(role)
    )
}

fn artifact_segment(
    artifact_type: ObservabilityArtifactType,
    artifact_number: ItemNumber,
) -> String {
    format!(
        "artifact:{}:{}",
        artifact_type.as_str(),
        artifact_number.get()
    )
}

fn length_prefixed(label: &str, value: &str) -> String {
    format!("{label}:{}:{value}", value.len())
}

fn length_value(value: &str) -> String {
    format!("{}:{value}", value.len())
}

/// Stable JSON event renderer backed by a sorted field map.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuredEvent {
    fields: BTreeMap<String, Value>,
}

impl StructuredEvent {
    /// Starts an event with the mandatory `event` field.
    pub fn new(event: impl Into<String>) -> Self {
        let mut fields = BTreeMap::new();
        fields.insert("event".to_string(), Value::String(event.into()));
        Self { fields }
    }

    /// Adds a string field.
    pub fn string(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.fields.insert(key.into(), Value::String(value.into()));
        self
    }

    /// Adds an optional string field, omitting absent values.
    pub fn optional_string(mut self, key: impl Into<String>, value: Option<String>) -> Self {
        if let Some(value) = value {
            self.fields.insert(key.into(), Value::String(value));
        }
        self
    }

    /// Adds an unsigned number field.
    pub fn number(mut self, key: impl Into<String>, value: u64) -> Self {
        self.fields.insert(key.into(), Value::Number(value.into()));
        self
    }

    /// Adds a boolean field.
    pub fn boolean(mut self, key: impl Into<String>, value: bool) -> Self {
        self.fields.insert(key.into(), Value::Bool(value));
        self
    }

    /// Adds a string-array field.
    pub fn string_array<I, S>(mut self, key: impl Into<String>, values: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.fields.insert(
            key.into(),
            Value::Array(
                values
                    .into_iter()
                    .map(|value| Value::String(value.into()))
                    .collect(),
            ),
        );
        self
    }

    /// Adds a pre-built JSON value field.
    pub fn json(mut self, key: impl Into<String>, value: Value) -> Self {
        self.fields.insert(key.into(), value);
        self
    }

    /// Renders one stable compact JSON event line.
    pub fn render(&self) -> String {
        serde_json::to_string(&self.fields).expect("event fields serialize")
    }
}

/// Startup/capability summary for a worker process.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerCapabilitySummary {
    /// Worker kind such as `role` or `mechanical`.
    pub worker_kind: String,
    /// Stable worker name.
    pub worker: String,
    /// Workflow role, when this is a role worker.
    pub role: Option<String>,
    /// Resolved repository display paths.
    pub repositories: Vec<String>,
    /// Responder mode such as `process` or `none`.
    pub responder_mode: String,
    /// Authorized workflow action names.
    pub authorized_actions: Vec<String>,
    /// Bound external-tool ids visible to the responder.
    pub bound_external_tool_ids: Vec<String>,
}

/// Renders a safe worker startup/capability event.
pub fn render_worker_capability_event(summary: &WorkerCapabilitySummary) -> String {
    StructuredEvent::new("worker_capabilities")
        .string("worker_kind", summary.worker_kind.clone())
        .string("worker", summary.worker.clone())
        .optional_string("role", summary.role.clone())
        .string_array("repositories", summary.repositories.clone())
        .string("responder_mode", summary.responder_mode.clone())
        .number(
            "authorized_action_count",
            saturating_u64(summary.authorized_actions.len()),
        )
        .string_array("authorized_actions", summary.authorized_actions.clone())
        .number(
            "available_external_tool_count",
            saturating_u64(summary.bound_external_tool_ids.len()),
        )
        .string_array(
            "available_external_tools",
            summary.bound_external_tool_ids.clone(),
        )
        .render()
}

fn saturating_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

/// Returns a one-line, character-bounded preview of arbitrary text.
pub fn bounded_preview(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= max_chars {
        return normalized;
    }
    if max_chars == 1 {
        return "…".to_string();
    }
    let mut preview = normalized.chars().take(max_chars - 1).collect::<String>();
    preview.push('…');
    preview
}

/// Returns a bounded preview, replacing secret-like text with [`REDACTED`].
pub fn redacted_preview(text: &str, max_chars: usize) -> String {
    if contains_secret_like(text) {
        REDACTED.to_string()
    } else {
        bounded_preview(text, max_chars)
    }
}

/// Converts possibly non-UTF-8 bytes to a redacted, bounded preview.
pub fn redacted_lossy_preview(bytes: &[u8], max_chars: usize) -> String {
    let text = String::from_utf8_lossy(bytes);
    redacted_preview(&text, max_chars)
}

fn contains_secret_like(text: &str) -> bool {
    let lowered = text.to_ascii_lowercase();
    [
        "token=",
        "token:",
        "password=",
        "password:",
        "secret=",
        "secret:",
        "authorization=",
        "authorization:",
        "bearer ",
        "api_key=",
        "api-key=",
        "auth=",
        "-----begin ",
    ]
    .iter()
    .any(|needle| lowered.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_item_identity_renders_stable_fields() {
        let identity = WorkItemIdentity::new(
            &RepositoryId::new("forgejo:acme/service"),
            &RoleId::new("banana"),
            &QueueId::new("todo"),
            ArtifactSource::Issue {
                number: ItemNumber::new(7),
            },
            &ArtifactKindId::new("task"),
        );

        assert_eq!(identity.artifact_type.as_str(), "issue");
        assert_eq!(
            identity.work_item_id,
            "work-item/repo:20:forgejo:acme/service/artifact:issue:7/kind:4:task/queue:4:todo/role:6:banana"
        );
        assert_eq!(
            identity.decision_id,
            "decision/work-item/repo:20:forgejo:acme/service/artifact:issue:7/kind:4:task/queue:4:todo/role:6:banana"
        );
        let json = identity.to_json();
        assert_eq!(json["repo"], "forgejo:acme/service");
        assert_eq!(json["role"], "banana");
        assert_eq!(json["queue"], "todo");
        assert_eq!(json["artifact_type"], "issue");
        assert_eq!(json["artifact_number"], 7);
        assert_eq!(json["artifact_kind"], "task");
        assert!(json.get("tick_id").is_none());
    }

    #[test]
    fn work_item_identity_can_include_tick_id() {
        let identity = WorkItemIdentity::new(
            &RepositoryId::new("repo-1"),
            &RoleId::new("engineer"),
            &QueueId::new("ready"),
            ArtifactSource::PullRequest {
                number: ItemNumber::new(2),
            },
            &ArtifactKindId::new("implementation_pr"),
        )
        .with_tick_id("tick-42");

        assert_eq!(identity.to_json()["tick_id"], "tick-42");
        assert!(identity.decision_id.contains("tick:7:tick-42"));
        assert_eq!(identity.to_json()["artifact_type"], "pull_request");
    }

    #[test]
    fn structured_event_output_is_stable_json() {
        let rendered = StructuredEvent::new("example")
            .string("zeta", "last")
            .number("count", 2)
            .boolean("ok", true)
            .string_array("items", ["b", "a"])
            .render();

        assert_eq!(
            rendered,
            r#"{"count":2,"event":"example","items":["b","a"],"ok":true,"zeta":"last"}"#
        );
    }

    #[test]
    fn preview_truncates_on_character_boundaries() {
        assert_eq!(
            bounded_preview("hello\nthere   friend", 14),
            "hello there f…"
        );
        assert_eq!(bounded_preview("éclair", 5), "écla…");
    }

    #[test]
    fn secret_like_previews_are_excluded() {
        assert_eq!(
            redacted_preview("TEMPER_FORGEJO_TOKEN=super-secret", 80),
            REDACTED
        );
        assert_eq!(
            redacted_lossy_preview(b"Authorization: bearer abc123", 80),
            REDACTED
        );
        assert_eq!(redacted_preview("ordinary reason", 80), "ordinary reason");
    }

    #[test]
    fn worker_capability_event_excludes_command_details() {
        let rendered = render_worker_capability_event(&WorkerCapabilitySummary {
            worker_kind: "role".to_string(),
            worker: "multi-role:banana".to_string(),
            role: Some("banana".to_string()),
            repositories: vec!["acme/service".to_string()],
            responder_mode: "process".to_string(),
            authorized_actions: vec!["advance".to_string()],
            bound_external_tool_ids: vec!["coding_workspace".to_string()],
        });

        assert!(rendered.contains(r#""event":"worker_capabilities""#));
        assert!(rendered.contains(r#""authorized_actions":["advance"]"#));
        assert!(rendered.contains(r#""available_external_tools":["coding_workspace"]"#));
        assert!(!rendered.contains("--auth"));
        assert!(!rendered.contains("token"));
    }
}
