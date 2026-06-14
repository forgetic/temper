//! Minimal JSON process protocols for external Temper responders.
//!
//! This crate contains serialization-oriented data transfer objects and
//! validation helpers for responder processes that communicate with Temper only
//! through stdin/stdout JSON. It intentionally has no dependency on Temper
//! runtime, workflow, Forge, backend, or deployment crates.

pub mod interaction;
pub mod workflow_role;

pub use interaction::{
    ConversationId, ConversationProfileId, ConversationReply, ConversationRequest,
    ConversationTurn, ConversationTurnId, DETERMINISTIC_SLUG_RULE, InteractionProtocolError,
    IssueProposal, Participant, ParticipantKind, Proposal, ProposalId, ProposalKind,
    ProposalPayloadValidator, is_valid_deterministic_slug, is_valid_proposal_slug,
    validate_deterministic_slug, validate_proposal_ids, validate_proposal_slug, validate_proposals,
};
pub use workflow_role::{
    AuthorizedWorkflowAction, BoundExternalTool, WORKFLOW_ROLE_DECISION_NO_ACTION,
    WORKFLOW_ROLE_DECISION_PROTOCOL_VERSION, WorkflowEffect, WorkflowExternalToolManifest,
    WorkflowPromptManifest, WorkflowPromptSection, WorkflowReviewDecision,
    WorkflowRoleDecisionProtocolError, WorkflowRoleDecisionReply, WorkflowRoleDecisionRequest,
    WorkflowRoleManifest, WorkflowRolePromptExtension, WorkflowToolManifest,
};
