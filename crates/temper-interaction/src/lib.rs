//! Provider-neutral interactive conversation primitives for Temper.
//!
//! This crate defines the reusable interaction-plane core: user-defined profile
//! validation/compilation, Forge-backed transcript sessions, durable proposal
//! snapshots, manifest-driven acceptance, and a provider-neutral process
//! responder adapter. It consumes and re-exports the conversation ids,
//! request/reply DTOs, and inert proposal DTOs from `temper-process-protocol` so
//! responder processes can depend on the wire contract without this runtime
//! crate. This crate has no workflow, runner, production, or LLM-provider
//! dependencies.

pub mod acceptance;
#[cfg(test)]
mod acceptance_tests;
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
pub mod proposal_state;
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

pub use acceptance::{
    AcceptanceExecutor, AcceptanceOutcome, AcceptanceRequest, AcceptedTarget,
    IssueAcceptanceOutcome, find_issue_by_marker, render_acceptance_marker,
    render_filed_issue_body,
};
pub use agent::InteractiveResponder;
pub use compile::{
    AcceptanceManifest, CommandActionManifest, CommandManifest, CompiledInteractionSpec,
    CompiledProfileManifest, ProfileManifest, ProposalManifest, ProposalPayloadValidator,
    ResponderManifest, TranscriptManifest, compile,
};
pub use error::InteractionError;
pub use ids::{AcceptanceActionId, CommandId, InteractionSpecId, ResponderId};
pub use process::{ProcessResponder, ProcessResponderConfig};
pub use proposal::{
    IssueIntakeAcceptanceConfig, IssueProposal, Proposal, accept_issue_intake_proposal,
    validate_proposal_ids, validate_proposals,
};
pub use proposal_state::{
    latest_proposals_from_turns, parse_proposal_snapshot_marker, render_agent_reply_comment,
    render_agent_reply_comment_with_proposals, render_proposal_snapshot_marker,
    strip_proposal_snapshot_marker,
};
pub use session::{ForgeInteractionSession, ForgeSessionConfig, ForgeSessionOpenOptions};
pub use spec::{
    RawAcceptProposalCommandAction, RawAcceptanceActionDeclaration, RawAcceptanceEffect,
    RawAcceptancePolicy, RawBacklinkPolicy, RawInteractionSpec, RawInteractiveProfile,
    RawParticipantDeclaration, RawParticipants, RawProposalKindDeclaration,
    RawResponderDeclaration, RawTranscriptPolicy, RawTransportCommandAction,
    RawTransportCommandDeclaration,
};
pub use temper_process_protocol::interaction::InteractionProtocolError;
pub use transcript::{
    DEFAULT_RECENT_TURN_LIMIT, ForgeTranscript, ForgeTranscriptConfig, ForgeTranscriptOpenOptions,
    append_marker, issue_url, parse_marker_value, parse_transcript_session_key,
    render_filing_marker, render_transcript_marker, validate_marker_namespace,
};
pub use transport::{
    AcceptProposalCommand, AcceptedProposalTarget, ConversationEvent, ConversationEventKind,
    ConversationEventLog, ConversationEventPayload, ConversationSnapshot,
    ConversationTranscriptRef, ListLatestProposalsCommand, OpenConversationCommand,
    SendHumanTurnCommand, SendHumanTurnResult,
};
pub use types::{
    ConversationId, ConversationProfileId, ConversationReply, ConversationRequest,
    ConversationTurn, ConversationTurnId, DETERMINISTIC_SLUG_RULE, Participant, ParticipantKind,
    ProposalId, ProposalKind, is_valid_deterministic_slug, is_valid_proposal_slug,
    validate_deterministic_slug, validate_proposal_slug,
};
pub use validate::{
    InteractionSpecDiagnostic, InteractionSpecReferenceSite, InteractionSpecSeverity,
    InteractionSpecSymbolKind, InteractionSpecValidationErrors,
    validate as validate_interaction_spec,
};
pub use validated::{
    AcceptanceEffect, AcceptancePolicy, AddTranscriptCommentEffect, BacklinkPolicy,
    CreateIssueEffect, ProposalPayloadContract, ResponderProtocol, TranscriptLabelPolicy,
    TranscriptTargetKind, TransportCommandAction, ValidatedAcceptanceActionDeclaration,
    ValidatedInteractionSpec, ValidatedInteractiveProfile, ValidatedParticipants,
    ValidatedProposalKindDeclaration, ValidatedResponderDeclaration, ValidatedTranscriptPolicy,
    ValidatedTransportCommandDeclaration,
};
