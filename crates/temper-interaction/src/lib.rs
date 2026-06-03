//! Provider-neutral interactive conversation primitives for Temper.
//!
//! This crate defines the reusable interaction-plane core: typed conversation
//! ids, responder request/reply types, inert proposals, user-defined profile
//! validation/compilation, Forge-backed transcript sessions, explicit idempotent
//! issue-proposal acceptance, and a provider-neutral process responder adapter.
//! Responder processes exchange the same serialized
//! request/reply types while transcript and acceptance code own durable Forge
//! state. This crate has no workflow, runner, production, or LLM-provider
//! dependencies.

pub mod agent;
pub mod compile;
#[cfg(test)]
mod compile_tests;
mod error;
pub mod ids;
pub mod process;
#[cfg(test)]
mod process_tests;
pub mod proposal;
pub mod session;
pub mod spec;
#[cfg(test)]
mod spec_tests;
#[cfg(test)]
mod tests;
pub mod transcript;
pub mod transport;
pub mod types;
pub mod validate;
pub mod validated;

pub use agent::InteractiveResponder;
pub use compile::{
    compile, AcceptanceManifest, CommandActionManifest, CommandManifest, CompiledInteractionSpec,
    CompiledProfileManifest, ProfileManifest, ProposalManifest, ProposalPayloadValidator,
    ResponderManifest, TranscriptManifest,
};
pub use error::InteractionError;
pub use ids::{AcceptanceActionId, CommandId, InteractionSpecId, ResponderId};
pub use process::{ProcessResponder, ProcessResponderConfig};
pub use proposal::{
    accept_issue_intake_proposal, find_issue_by_marker, render_filed_issue_body,
    validate_proposal_ids, validate_proposals, IssueAcceptanceOutcome, IssueIntakeAcceptanceConfig,
    IssueProposal, Proposal,
};
pub use session::{
    render_agent_reply_comment, ForgeInteractionSession, ForgeSessionConfig,
    ForgeSessionOpenOptions,
};
pub use spec::{
    RawAcceptProposalCommandAction, RawAcceptanceActionDeclaration, RawAcceptanceEffect,
    RawAcceptancePolicy, RawBacklinkPolicy, RawInteractionSpec, RawInteractiveProfile,
    RawParticipantDeclaration, RawParticipants, RawProposalKindDeclaration,
    RawResponderDeclaration, RawTranscriptPolicy, RawTransportCommandAction,
    RawTransportCommandDeclaration,
};
pub use transcript::{
    append_marker, issue_url, parse_marker_value, parse_transcript_session_key,
    render_filing_marker, render_transcript_marker, validate_marker_namespace, ForgeTranscript,
    ForgeTranscriptConfig, ForgeTranscriptOpenOptions, DEFAULT_RECENT_TURN_LIMIT,
};
pub use transport::{
    AcceptProposalCommand, AcceptedProposalTarget, ConversationEvent, ConversationEventKind,
    ConversationEventLog, ConversationEventPayload, ConversationSnapshot,
    ConversationTranscriptRef, ListLatestProposalsCommand, OpenConversationCommand,
    SendHumanTurnCommand, SendHumanTurnResult,
};
pub use types::{
    is_valid_deterministic_slug, is_valid_proposal_slug, validate_deterministic_slug,
    validate_proposal_slug, ConversationId, ConversationProfileId, ConversationReply,
    ConversationRequest, ConversationTurn, ConversationTurnId, Participant, ParticipantKind,
    ProposalId, ProposalKind, DETERMINISTIC_SLUG_RULE,
};
pub use validate::{
    validate as validate_interaction_spec, InteractionSpecDiagnostic, InteractionSpecReferenceSite,
    InteractionSpecSeverity, InteractionSpecSymbolKind, InteractionSpecValidationErrors,
};
pub use validated::{
    AcceptanceEffect, AcceptancePolicy, BacklinkPolicy, CreateIssueEffect, ProposalPayloadContract,
    ResponderProtocol, TranscriptLabelPolicy, TranscriptTargetKind, TransportCommandAction,
    ValidatedAcceptanceActionDeclaration, ValidatedInteractionSpec, ValidatedInteractiveProfile,
    ValidatedParticipants, ValidatedProposalKindDeclaration, ValidatedResponderDeclaration,
    ValidatedTranscriptPolicy, ValidatedTransportCommandDeclaration,
};
