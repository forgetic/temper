//! Re-exports and adapters for the workflow-role process protocol.
//!
//! The JSON protocol DTOs live in `temper-process-protocol` so external
//! responders can depend on the wire contract without depending on Temper's
//! runner, workflow engine, Forge interface, or concrete backends. This module
//! keeps the historical `temper_runner::...` exports and converts runtime
//! workflow manifests into the protocol mirror before invoking a process.

use temper_forge_model::ReviewDecision;
use temper_workflow::{Effect, RoleManifest, ToolManifest};

pub use temper_process_protocol::workflow_role::{
    AuthorizedWorkflowAction, BoundExternalTool, WORKFLOW_ROLE_DECISION_NO_ACTION,
    WORKFLOW_ROLE_DECISION_PROTOCOL_VERSION, WorkflowEffect, WorkflowExternalToolManifest,
    WorkflowPromptManifest, WorkflowPromptSection, WorkflowReviewDecision,
    WorkflowRoleDecisionProtocolError, WorkflowRoleDecisionReply, WorkflowRoleDecisionRequest,
    WorkflowRoleManifest, WorkflowRolePromptExtension, WorkflowToolManifest,
};

pub(crate) fn workflow_role_manifest_from_runtime(manifest: &RoleManifest) -> WorkflowRoleManifest {
    WorkflowRoleManifest {
        id: manifest.id.to_string(),
        charter: manifest.charter.clone(),
        prompt_extension: WorkflowRolePromptExtension {
            guidance: manifest.prompt_extension.guidance.clone(),
            tool_guidance: manifest.prompt_extension.tool_guidance.clone(),
        },
        concurrency: manifest.concurrency,
        queues: manifest.queues.iter().map(ToString::to_string).collect(),
        authority: manifest.authority.iter().map(ToString::to_string).collect(),
        tools: manifest
            .tools
            .iter()
            .map(workflow_tool_from_runtime)
            .collect(),
        external_tools: manifest
            .external_tools
            .iter()
            .map(|tool| WorkflowExternalToolManifest {
                id: tool.id.to_string(),
                description: tool.description.clone(),
                required: tool.required,
                constraints: tool.constraints.clone(),
                guidance: tool.guidance.clone(),
            })
            .collect(),
        prompt: WorkflowPromptManifest {
            role: manifest.prompt.role.to_string(),
            sections: manifest
                .prompt
                .sections
                .iter()
                .map(|section| WorkflowPromptSection {
                    heading: section.heading.clone(),
                    lines: section.lines.clone(),
                })
                .collect(),
        },
    }
}

fn workflow_tool_from_runtime(tool: &ToolManifest) -> WorkflowToolManifest {
    WorkflowToolManifest {
        name: tool.name.clone(),
        transition: tool.transition.to_string(),
        artifact: tool.artifact.to_string(),
        requires_gates: tool
            .requires_gates
            .iter()
            .map(ToString::to_string)
            .collect(),
        effects: tool
            .effects
            .iter()
            .map(workflow_effect_from_runtime)
            .collect(),
    }
}

fn workflow_effect_from_runtime(effect: &Effect) -> WorkflowEffect {
    match effect {
        Effect::AddLabel(label) => WorkflowEffect::AddLabel(label.to_string()),
        Effect::RemoveLabel(label) => WorkflowEffect::RemoveLabel(label.to_string()),
        Effect::SetAssignee(role) => WorkflowEffect::SetAssignee(role.to_string()),
        Effect::RemoveAssignee(role) => WorkflowEffect::RemoveAssignee(role.to_string()),
        Effect::CreateComment { body } => WorkflowEffect::CreateComment { body: body.clone() },
        Effect::CreatePullRequest { correlation_key } => WorkflowEffect::CreatePullRequest {
            correlation_key: correlation_key.clone(),
        },
        Effect::RequestReviewers { roles } => WorkflowEffect::RequestReviewers {
            roles: roles.iter().map(ToString::to_string).collect(),
        },
        Effect::SubmitReview { decision } => WorkflowEffect::SubmitReview {
            decision: workflow_review_decision_from_runtime(*decision),
        },
        Effect::SetBody { correlation_key } => WorkflowEffect::SetBody {
            correlation_key: correlation_key.clone(),
        },
        Effect::AttachReview {
            decision,
            correlation_key,
        } => WorkflowEffect::AttachReview {
            decision: workflow_review_decision_from_runtime(*decision),
            correlation_key: correlation_key.clone(),
        },
        Effect::CreateIssues { correlation_key } => WorkflowEffect::CreateIssues {
            correlation_key: correlation_key.clone(),
        },
        Effect::MergePullRequest => WorkflowEffect::MergePullRequest,
    }
}

fn workflow_review_decision_from_runtime(decision: ReviewDecision) -> WorkflowReviewDecision {
    match decision {
        ReviewDecision::Approved => WorkflowReviewDecision::Approved,
        ReviewDecision::ChangesRequested => WorkflowReviewDecision::ChangesRequested,
        ReviewDecision::Commented => WorkflowReviewDecision::Commented,
        ReviewDecision::Pending => WorkflowReviewDecision::Pending,
    }
}

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
