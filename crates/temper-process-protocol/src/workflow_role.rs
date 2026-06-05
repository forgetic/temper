use std::error::Error;
use std::fmt;

use serde_json::Value;

/// Current workflow-role decision process protocol version.
pub const WORKFLOW_ROLE_DECISION_PROTOCOL_VERSION: u32 = 1;
/// Sentinel action for a safe no-op decision.
pub const WORKFLOW_ROLE_DECISION_NO_ACTION: &str = "no_action";

/// Portable pull-request review decision shape used inside workflow manifests.
#[derive(Copy, Clone, Debug, serde::Deserialize, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowReviewDecision {
    Approved,
    ChangesRequested,
    Commented,
    Pending,
}

/// A workflow transition effect as serialized in a role manifest.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowEffect {
    AddLabel(String),
    RemoveLabel(String),
    SetAssignee(String),
    RemoveAssignee(String),
    CreateComment { body: String },
    CreatePullRequest { correlation_key: Option<String> },
    RequestReviewers { roles: Vec<String> },
    SubmitReview { decision: WorkflowReviewDecision },
    MergePullRequest,
}

/// User-authored prompt prose attached to a workflow role.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct WorkflowRolePromptExtension {
    /// Behavioral guidance for role judgment. It is advisory only.
    pub guidance: Option<String>,
    /// User guidance for using declared tools. It is advisory only.
    pub tool_guidance: Option<String>,
}

/// Deterministic prompt sections for one workflow role.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct WorkflowPromptManifest {
    pub role: String,
    pub sections: Vec<WorkflowPromptSection>,
}

impl WorkflowPromptManifest {
    /// Returns the section with the given heading, if present.
    pub fn section(&self, heading: &str) -> Option<&WorkflowPromptSection> {
        self.sections
            .iter()
            .find(|section| section.heading == heading)
    }

    /// Returns the mutable section with the given heading, if present.
    pub fn section_mut(&mut self, heading: &str) -> Option<&mut WorkflowPromptSection> {
        self.sections
            .iter_mut()
            .find(|section| section.heading == heading)
    }

    /// Renders the sections into a stable plain-text prompt.
    pub fn render(&self) -> String {
        let mut out = String::new();
        for (index, section) in self.sections.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            out.push_str("## ");
            out.push_str(&section.heading);
            out.push('\n');
            for line in &section.lines {
                out.push_str(line);
                out.push('\n');
            }
        }
        out
    }
}

/// One headed prompt block.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct WorkflowPromptSection {
    pub heading: String,
    pub lines: Vec<String>,
}

/// One intent-level workflow tool exposed to an external role decision process.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct WorkflowToolManifest {
    /// Intent-level tool name; equal to the transition id in today's compiler output.
    pub name: String,
    /// Transition this tool applies.
    pub transition: String,
    /// Artifact kind the tool operates on.
    pub artifact: String,
    /// Gates that must be satisfied before the tool may run.
    pub requires_gates: Vec<String>,
    /// Effects the tool applies when it runs.
    pub effects: Vec<WorkflowEffect>,
}

/// User-declared non-workflow tool metadata as serialized in the role manifest.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct WorkflowExternalToolManifest {
    pub id: String,
    pub description: String,
    pub required: bool,
    pub constraints: Vec<String>,
    pub guidance: Option<String>,
}

/// Serialized role manifest visible to an external workflow-role responder.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct WorkflowRoleManifest {
    pub id: String,
    pub charter: Option<String>,
    pub prompt_extension: WorkflowRolePromptExtension,
    pub concurrency: Option<u32>,
    pub queues: Vec<String>,
    pub authority: Vec<String>,
    pub tools: Vec<WorkflowToolManifest>,
    pub external_tools: Vec<WorkflowExternalToolManifest>,
    pub prompt: WorkflowPromptManifest,
}

/// One authorized workflow action exposed to a role decision engine.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorizedWorkflowAction {
    /// Action string the decision reply may choose; equal to the manifest tool
    /// name and transition id in today's compiler output.
    pub action: String,
    /// Transition the action maps to.
    pub transition: String,
    /// Workflow artifact kind the action can operate on.
    pub artifact: String,
    /// Gates that must be satisfied before Temper can execute the transition.
    #[serde(default)]
    pub requires_gates: Vec<String>,
}

impl AuthorizedWorkflowAction {
    fn from_tool(tool: &WorkflowToolManifest) -> Self {
        Self {
            action: tool.name.clone(),
            transition: tool.transition.clone(),
            artifact: tool.artifact.clone(),
            requires_gates: tool.requires_gates.clone(),
        }
    }
}

/// External tool metadata that survived declaration + runner binding validation.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct BoundExternalTool {
    pub id: String,
    pub description: String,
    pub required: bool,
    pub constraints: Vec<String>,
    pub guidance: Option<String>,
    pub provider: String,
}

/// Request sent by Temper to an external workflow-role decision process.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowRoleDecisionRequest {
    /// Protocol version; currently [`WORKFLOW_ROLE_DECISION_PROTOCOL_VERSION`].
    pub protocol_version: u32,
    /// Workflow id/name from the compiled workflow.
    pub workflow_id: String,
    /// Serialized compiled role manifest Temper is enforcing for this worker.
    pub role_manifest: WorkflowRoleManifest,
    /// Fresh work-item context JSON assembled by the runner. It must not contain
    /// Forge credentials, provider secrets, or mutation handles.
    pub work_item_context: Value,
    /// Compact action list for models that should not inspect the full manifest.
    pub authorized_actions: Vec<AuthorizedWorkflowAction>,
    /// User-declared external tools that are both declared and runner-bound for
    /// this role. These are metadata only; the decision process cannot execute
    /// them through this protocol.
    #[serde(default)]
    pub available_external_tools: Vec<BoundExternalTool>,
}

impl WorkflowRoleDecisionRequest {
    /// Builds a version-1 request and derives the compact action list from the
    /// supplied serialized role manifest.
    pub fn new(
        workflow_id: impl Into<String>,
        role_manifest: WorkflowRoleManifest,
        work_item_context: Value,
        available_external_tools: Vec<BoundExternalTool>,
    ) -> Self {
        let authorized_actions = role_manifest
            .tools
            .iter()
            .map(AuthorizedWorkflowAction::from_tool)
            .collect();
        Self {
            protocol_version: WORKFLOW_ROLE_DECISION_PROTOCOL_VERSION,
            workflow_id: workflow_id.into(),
            role_manifest,
            work_item_context,
            authorized_actions,
            available_external_tools,
        }
    }

    /// Returns whether `action` is either `no_action` or one of the manifest
    /// tool names in this request.
    pub fn action_is_authorized(&self, action: &str) -> bool {
        action == WORKFLOW_ROLE_DECISION_NO_ACTION
            || self
                .authorized_actions
                .iter()
                .any(|candidate| candidate.action == action)
    }

    /// Validates a reply against this request's protocol version and action
    /// authority. Process adapters should call this before running any tool.
    pub fn validate_reply(
        &self,
        reply: &WorkflowRoleDecisionReply,
    ) -> Result<(), WorkflowRoleDecisionProtocolError> {
        if reply.protocol_version != self.protocol_version {
            return Err(WorkflowRoleDecisionProtocolError::VersionMismatch {
                expected: self.protocol_version,
                actual: reply.protocol_version,
            });
        }
        if !self.action_is_authorized(&reply.action) {
            return Err(WorkflowRoleDecisionProtocolError::UnauthorizedAction {
                action: reply.action.clone(),
            });
        }
        Ok(())
    }
}

/// Reply returned by an external workflow-role decision process.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowRoleDecisionReply {
    /// Protocol version echoed by the responder.
    pub protocol_version: u32,
    /// One manifest tool name from the request, or [`WORKFLOW_ROLE_DECISION_NO_ACTION`].
    pub action: String,
    /// Short rationale for logs and operator debugging. It does not grant
    /// authority and is not interpreted by the runner.
    #[serde(default)]
    pub reason: String,
}

impl WorkflowRoleDecisionReply {
    /// Builds a safe no-op reply.
    pub fn no_action(reason: impl Into<String>) -> Self {
        Self {
            protocol_version: WORKFLOW_ROLE_DECISION_PROTOCOL_VERSION,
            action: WORKFLOW_ROLE_DECISION_NO_ACTION.to_string(),
            reason: reason.into(),
        }
    }

    /// Builds a reply choosing one manifest action.
    pub fn action(action: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            protocol_version: WORKFLOW_ROLE_DECISION_PROTOCOL_VERSION,
            action: action.into(),
            reason: reason.into(),
        }
    }

    /// Returns whether this reply deliberately chooses no workflow action.
    pub fn is_no_action(&self) -> bool {
        self.action == WORKFLOW_ROLE_DECISION_NO_ACTION
    }
}

/// Validation failure for a workflow-role decision reply.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkflowRoleDecisionProtocolError {
    /// The reply used a different protocol version than the request.
    VersionMismatch { expected: u32, actual: u32 },
    /// The reply selected an action outside the manifest authority.
    UnauthorizedAction { action: String },
}

impl fmt::Display for WorkflowRoleDecisionProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::VersionMismatch { expected, actual } => write!(
                formatter,
                "workflow-role decision protocol version mismatch: expected {expected}, got {actual}"
            ),
            Self::UnauthorizedAction { action } => write!(
                formatter,
                "workflow-role decision selected unauthorized action `{action}`"
            ),
        }
    }
}

impl Error for WorkflowRoleDecisionProtocolError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workflow_role_decision_fixtures_round_trip_and_validate() {
        let request_json = include_str!("../fixtures/workflow-role-decision-request.json");
        let request: WorkflowRoleDecisionRequest =
            serde_json::from_str(request_json).expect("request fixture parses");
        assert_eq!(
            request.protocol_version,
            WORKFLOW_ROLE_DECISION_PROTOCOL_VERSION
        );
        assert_eq!(request.workflow_id, "generic-agent-test");
        assert_eq!(request.role_manifest.id.as_str(), "banana");
        assert_eq!(
            request.work_item_context["observability"]["repo"],
            "forgejo:acme/service"
        );
        assert_eq!(
            request.work_item_context["observability"]["artifact_type"],
            "issue"
        );
        assert!(request.action_is_authorized("advance"));
        assert!(request.action_is_authorized(WORKFLOW_ROLE_DECISION_NO_ACTION));
        assert!(!request.action_is_authorized("delete_everything"));
        assert!(request
            .role_manifest
            .prompt
            .render()
            .contains("Role: banana"));

        let reply_json = include_str!("../fixtures/workflow-role-decision-reply.json");
        let reply: WorkflowRoleDecisionReply =
            serde_json::from_str(reply_json).expect("reply fixture parses");
        request
            .validate_reply(&reply)
            .expect("fixture reply is authorized");

        let encoded = serde_json::to_string_pretty(&request).expect("request serializes");
        let decoded: WorkflowRoleDecisionRequest =
            serde_json::from_str(&encoded).expect("request round-trips");
        assert_eq!(decoded, request);
    }

    #[test]
    fn workflow_role_decision_rejects_wrong_version_or_action() {
        let request: WorkflowRoleDecisionRequest = serde_json::from_str(include_str!(
            "../fixtures/workflow-role-decision-request.json"
        ))
        .expect("request fixture parses");

        let wrong_version = WorkflowRoleDecisionReply {
            protocol_version: 999,
            ..WorkflowRoleDecisionReply::no_action("old responder")
        };
        assert!(matches!(
            request.validate_reply(&wrong_version),
            Err(WorkflowRoleDecisionProtocolError::VersionMismatch { .. })
        ));

        let unauthorized = WorkflowRoleDecisionReply::action("delete_everything", "not allowed");
        assert!(matches!(
            request.validate_reply(&unauthorized),
            Err(WorkflowRoleDecisionProtocolError::UnauthorizedAction { .. })
        ));
    }
}
