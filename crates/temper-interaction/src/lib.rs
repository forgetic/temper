//! Provider-neutral interactive conversation primitives for Temper.
//!
//! This crate defines the reusable interaction-plane core: typed conversation
//! ids, responder request/reply types, inert proposals, Forge-backed transcript
//! sessions, and explicit idempotent issue-proposal acceptance. Responder
//! requests and replies remain serializable for future process protocols, while
//! transcript and acceptance code owns durable Forge state. This crate has no
//! workflow, runner, production, or LLM-provider dependencies.

pub mod agent;
mod error;
pub mod proposal;
pub mod session;
#[cfg(test)]
mod tests;
pub mod transcript;
pub mod types;

pub use agent::InteractiveResponder;
pub use error::InteractionError;
pub use proposal::{
    accept_issue_intake_proposal, find_issue_by_marker, render_filed_issue_body,
    validate_proposal_ids, IssueAcceptanceOutcome, IssueIntakeAcceptanceConfig, IssueProposal,
    Proposal,
};
pub use session::{
    render_agent_reply_comment, ForgeInteractionSession, ForgeSessionConfig,
    ForgeSessionOpenOptions,
};
pub use transcript::{
    append_marker, issue_url, parse_marker_value, parse_transcript_session_key,
    render_filing_marker, render_transcript_marker, validate_marker_namespace, ForgeTranscript,
    ForgeTranscriptConfig, ForgeTranscriptOpenOptions, DEFAULT_RECENT_TURN_LIMIT,
};
pub use types::{
    is_valid_deterministic_slug, is_valid_proposal_slug, validate_deterministic_slug,
    validate_proposal_slug, ConversationId, ConversationProfileId, ConversationReply,
    ConversationRequest, ConversationTurn, ConversationTurnId, Participant, ParticipantKind,
    ProposalId, ProposalKind,
};
