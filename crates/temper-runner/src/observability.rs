//! Provider-neutral observability identity and structured event helpers.
//!
//! This module only formats context and log records. It owns no telemetry
//! backend, reads no provider secrets, and grants no workflow authority.

use std::collections::BTreeMap;

mod events;
mod identity;
mod redact;

pub use events::{
    ActionDispatchEvent, MechanicalReconciliationEvent, RoleDecisionReplyEvent,
    RoleDecisionRequestEvent, ScanSummaryEvent, TransitionExecutionEvent, WorkItemSelectedEvent,
    execution_error_diagnostic_classes, execution_error_failure_class,
    postcondition_outcome_for_error, render_action_dispatch_event,
    render_mechanical_reconciliation_event, render_role_decision_reply_event,
    render_role_decision_request_event, render_scan_summary_event,
    render_transition_execution_event, render_work_item_selected_event, workflow_effect_summary,
};
pub use identity::{ObservabilityArtifactType, WorkItemIdentity};
pub use redact::{REDACTED, bounded_preview, redacted_lossy_preview, redacted_preview};

use serde_json::Value;

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

#[cfg(test)]
mod tests {
    use super::*;

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
