//! Validated user-defined interaction profile model.
//!
//! A [`ValidatedInteractionSpec`] is the normalized form of a raw interaction
//! specification. It can only be produced by [`crate::validate::validate`] or
//! [`crate::spec::RawInteractionSpec::validate`]; its fields are crate-private so
//! downstream compiler/runtime APIs can require this type and trust that static
//! checks have already run.

use crate::ids::{AcceptanceActionId, CommandId, InteractionSpecId, ResponderId};
use crate::{ConversationProfileId, Participant, ProposalKind};

/// Interaction spec that passed static validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedInteractionSpec {
    pub(crate) id: InteractionSpecId,
    pub(crate) responders: Vec<ValidatedResponderDeclaration>,
    pub(crate) profiles: Vec<ValidatedInteractiveProfile>,
}

impl ValidatedInteractionSpec {
    /// Returns the interaction spec id.
    pub fn id(&self) -> &InteractionSpecId {
        &self.id
    }

    /// Returns declared responders in spec order.
    pub fn responders(&self) -> &[ValidatedResponderDeclaration] {
        &self.responders
    }

    /// Returns validated profiles in spec order.
    pub fn profiles(&self) -> &[ValidatedInteractiveProfile] {
        &self.profiles
    }

    /// Finds a profile by id.
    pub fn profile(&self, id: &ConversationProfileId) -> Option<&ValidatedInteractiveProfile> {
        self.profiles.iter().find(|profile| profile.id() == id)
    }
}

/// Validated interactive profile declaration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedInteractiveProfile {
    pub(crate) id: ConversationProfileId,
    pub(crate) transcript: ValidatedTranscriptPolicy,
    pub(crate) participants: ValidatedParticipants,
    pub(crate) responder: ResponderId,
    pub(crate) proposal_kinds: Vec<ValidatedProposalKindDeclaration>,
    pub(crate) commands: Vec<ValidatedTransportCommandDeclaration>,
    pub(crate) acceptance_actions: Vec<ValidatedAcceptanceActionDeclaration>,
}

impl ValidatedInteractiveProfile {
    /// Returns the profile id.
    pub fn id(&self) -> &ConversationProfileId {
        &self.id
    }

    /// Returns the transcript policy.
    pub fn transcript(&self) -> &ValidatedTranscriptPolicy {
        &self.transcript
    }

    /// Returns participant display policy.
    pub fn participants(&self) -> &ValidatedParticipants {
        &self.participants
    }

    /// Returns the responder id used by this profile.
    pub fn responder(&self) -> &ResponderId {
        &self.responder
    }

    /// Returns proposal kind declarations in profile order.
    pub fn proposal_kinds(&self) -> &[ValidatedProposalKindDeclaration] {
        &self.proposal_kinds
    }

    /// Returns command declarations in profile order.
    pub fn commands(&self) -> &[ValidatedTransportCommandDeclaration] {
        &self.commands
    }

    /// Returns acceptance action declarations in profile order.
    pub fn acceptance_actions(&self) -> &[ValidatedAcceptanceActionDeclaration] {
        &self.acceptance_actions
    }
}

/// Transcript backing target.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TranscriptTargetKind {
    /// Store transcripts as Forge issues.
    ForgeIssue,
}

/// How transcript labels are interpreted when creating or resuming transcripts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TranscriptLabelPolicy {
    /// The transcript issue must carry exactly the declared label set.
    Exact,
}

/// Validated transcript policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedTranscriptPolicy {
    pub(crate) target: TranscriptTargetKind,
    pub(crate) title_prefix: String,
    pub(crate) labels: Vec<String>,
    pub(crate) label_policy: TranscriptLabelPolicy,
    pub(crate) marker_namespace: String,
    pub(crate) recent_turn_limit: usize,
}

impl ValidatedTranscriptPolicy {
    /// Returns the transcript target.
    pub fn target(&self) -> TranscriptTargetKind {
        self.target
    }

    /// Returns the title prefix for created transcripts.
    pub fn title_prefix(&self) -> &str {
        &self.title_prefix
    }

    /// Returns the transcript label set.
    pub fn labels(&self) -> &[String] {
        &self.labels
    }

    /// Returns the transcript label matching policy.
    pub fn label_policy(&self) -> TranscriptLabelPolicy {
        self.label_policy
    }

    /// Returns the hidden marker namespace.
    pub fn marker_namespace(&self) -> &str {
        &self.marker_namespace
    }

    /// Returns the recent-turn limit.
    pub fn recent_turn_limit(&self) -> usize {
        self.recent_turn_limit
    }
}

/// Validated human and agent participant policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedParticipants {
    pub(crate) human: Participant,
    pub(crate) agent: Participant,
}

impl ValidatedParticipants {
    /// Returns the human participant representation.
    pub fn human(&self) -> &Participant {
        &self.human
    }

    /// Returns the agent participant representation.
    pub fn agent(&self) -> &Participant {
        &self.agent
    }
}

/// Responder process protocol/version intent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResponderProtocol {
    /// The v1 stdin/stdout JSON process protocol.
    ProcessV1,
}

/// Validated responder declaration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedResponderDeclaration {
    pub(crate) id: ResponderId,
    pub(crate) protocol: ResponderProtocol,
    pub(crate) required: bool,
}

impl ValidatedResponderDeclaration {
    /// Returns the responder id.
    pub fn id(&self) -> &ResponderId {
        &self.id
    }

    /// Returns the process protocol intent.
    pub fn protocol(&self) -> ResponderProtocol {
        self.protocol
    }

    /// Returns whether a deployment must bind this responder.
    pub fn required(&self) -> bool {
        self.required
    }
}

/// Built-in proposal payload contracts known to Temper.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProposalPayloadContract {
    /// Payload compatible with [`crate::IssueProposal`].
    IssueDraft,
}

/// Validated proposal kind declaration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedProposalKindDeclaration {
    pub(crate) id: ProposalKind,
    pub(crate) payload: ProposalPayloadContract,
}

impl ValidatedProposalKindDeclaration {
    /// Returns the proposal kind id.
    pub fn id(&self) -> &ProposalKind {
        &self.id
    }

    /// Returns the payload contract.
    pub fn payload(&self) -> ProposalPayloadContract {
        self.payload
    }
}

/// Validated transport command declaration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedTransportCommandDeclaration {
    pub(crate) id: CommandId,
    pub(crate) aliases: Vec<String>,
    pub(crate) action: TransportCommandAction,
}

impl ValidatedTransportCommandDeclaration {
    /// Returns the command id.
    pub fn id(&self) -> &CommandId {
        &self.id
    }

    /// Returns normalized command aliases.
    pub fn aliases(&self) -> &[String] {
        &self.aliases
    }

    /// Returns the action this command requests.
    pub fn action(&self) -> &TransportCommandAction {
        &self.action
    }
}

/// Validated command action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TransportCommandAction {
    /// Explicitly accept a proposal through a declared acceptance action.
    AcceptProposal {
        /// Proposal kind the command accepts.
        kind: ProposalKind,
        /// Acceptance action id to execute.
        acceptance_action: AcceptanceActionId,
    },
}

/// Explicit acceptance policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AcceptancePolicy {
    /// Human acceptance must arrive through one of the listed commands.
    Explicit { commands: Vec<CommandId> },
}

/// Validated accepted-proposal action declaration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedAcceptanceActionDeclaration {
    pub(crate) id: AcceptanceActionId,
    pub(crate) proposal_kind: ProposalKind,
    pub(crate) acceptance: AcceptancePolicy,
    pub(crate) idempotency_key_template: String,
    pub(crate) effects: Vec<AcceptanceEffect>,
}

impl ValidatedAcceptanceActionDeclaration {
    /// Returns the acceptance action id.
    pub fn id(&self) -> &AcceptanceActionId {
        &self.id
    }

    /// Returns the accepted proposal kind.
    pub fn proposal_kind(&self) -> &ProposalKind {
        &self.proposal_kind
    }

    /// Returns the explicit acceptance policy.
    pub fn acceptance(&self) -> &AcceptancePolicy {
        &self.acceptance
    }

    /// Returns the idempotency key template.
    pub fn idempotency_key_template(&self) -> &str {
        &self.idempotency_key_template
    }

    /// Returns the closed effect list.
    pub fn effects(&self) -> &[AcceptanceEffect] {
        &self.effects
    }
}

/// Closed set of effects accepted actions may execute.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AcceptanceEffect {
    /// Create a Forge issue from an accepted proposal.
    CreateIssue(CreateIssueEffect),
}

/// Validated create-issue effect.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreateIssueEffect {
    pub(crate) title: String,
    pub(crate) body_template: String,
    pub(crate) labels: Vec<String>,
    pub(crate) marker_namespace: String,
    pub(crate) backlink: Option<BacklinkPolicy>,
}

impl CreateIssueEffect {
    /// Returns the issue title template.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Returns the issue body template.
    pub fn body_template(&self) -> &str {
        &self.body_template
    }

    /// Returns labels applied to the created issue.
    pub fn labels(&self) -> &[String] {
        &self.labels
    }

    /// Returns the marker namespace used for idempotency.
    pub fn marker_namespace(&self) -> &str {
        &self.marker_namespace
    }

    /// Returns optional backlink metadata.
    pub fn backlink(&self) -> Option<&BacklinkPolicy> {
        self.backlink.as_ref()
    }
}

/// Validated transcript backlink metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BacklinkPolicy {
    pub(crate) label: String,
    pub(crate) url: String,
}

impl BacklinkPolicy {
    /// Returns the backlink label.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Returns the backlink URL template.
    pub fn url(&self) -> &str {
        &self.url
    }
}
