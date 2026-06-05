//! Interactive process protocol DTO re-exports.
//!
//! The conversation request/reply wire types and deterministic id helpers live
//! in `temper-process-protocol` so external responders can depend on the JSON
//! contract without pulling in Temper's Forge-backed interaction runtime.

pub use temper_process_protocol::interaction::{
    is_valid_deterministic_slug, is_valid_proposal_slug, validate_deterministic_slug,
    validate_proposal_slug, ConversationId, ConversationProfileId, ConversationReply,
    ConversationRequest, ConversationTurn, ConversationTurnId, Participant, ParticipantKind,
    ProposalId, ProposalKind, DETERMINISTIC_SLUG_RULE,
};
