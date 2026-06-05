//! Minimal JSON process protocols for external Temper responders.
//!
//! This crate contains serialization-oriented data transfer objects and
//! validation helpers for responder processes that communicate with Temper only
//! through stdin/stdout JSON. It intentionally has no dependency on Temper
//! runtime, workflow, Forge, backend, or deployment crates.

pub mod interaction;
pub mod workflow_role;

pub use interaction::{
    is_valid_deterministic_slug, is_valid_proposal_slug, validate_deterministic_slug,
    validate_proposal_ids, validate_proposal_slug, validate_proposals, ConversationId,
    ConversationProfileId, ConversationReply, ConversationRequest, ConversationTurn,
    ConversationTurnId, InteractionProtocolError, IssueProposal, Participant, ParticipantKind,
    Proposal, ProposalId, ProposalKind, ProposalPayloadValidator, DETERMINISTIC_SLUG_RULE,
};
pub use workflow_role::{
    AuthorizedWorkflowAction, BoundExternalTool, WorkflowEffect, WorkflowExternalToolManifest,
    WorkflowPromptManifest, WorkflowPromptSection, WorkflowReviewDecision,
    WorkflowRoleDecisionProtocolError, WorkflowRoleDecisionReply, WorkflowRoleDecisionRequest,
    WorkflowRoleManifest, WorkflowRolePromptExtension, WorkflowToolManifest,
    WORKFLOW_ROLE_DECISION_NO_ACTION, WORKFLOW_ROLE_DECISION_PROTOCOL_VERSION,
};
