//! Provider-neutral interactive conversation primitives for Temper.
//!
//! This crate defines the small domain model used by Temper's interaction
//! plane: typed conversation/profile/proposal identifiers, transcript turns,
//! and inert proposals. The request/reply/proposal structs are intentionally
//! serializable as the domain shape for future process responder protocols; the
//! object-safe responder trait is only the in-process adapter seam. This crate
//! has no Forge, workflow, runner, production, or LLM-provider dependencies.
//! Later crates may persist transcripts, accept proposals, or adapt concrete LLM
//! profiles using these types.

pub mod agent;
mod error;
pub mod proposal;
#[cfg(test)]
mod tests;
pub mod types;

pub use agent::InteractiveResponder;
pub use error::InteractionError;
pub use proposal::{validate_proposal_ids, IssueProposal, Proposal};
pub use types::{
    is_valid_deterministic_slug, is_valid_proposal_slug, validate_deterministic_slug,
    validate_proposal_slug, ConversationId, ConversationProfileId, ConversationReply,
    ConversationRequest, ConversationTurn, ConversationTurnId, Participant, ParticipantKind,
    ProposalId, ProposalKind,
};
