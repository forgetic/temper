//! Compilation of validated interaction specs into runtime manifests.
//!
//! Compilation is the bridge between a [`ValidatedInteractionSpec`] and generic
//! interaction runtimes/transports. It never opens a Forge backend, invokes a
//! responder, or applies acceptance effects; it only projects the already-
//! checked profile model into deterministic manifests that runtime code can
//! consume. Because the input is validated, compilation is infallible.

use crate::ids::{AcceptanceActionId, CommandId, InteractionSpecId, ResponderId};
use crate::proposal::{IssueProposal, Proposal};
use crate::validated::{
    AcceptanceEffect, AcceptancePolicy, ProposalPayloadContract, ResponderProtocol,
    TranscriptLabelPolicy, TranscriptTargetKind, TransportCommandAction, ValidatedInteractionSpec,
    ValidatedInteractiveProfile,
};
use crate::{ConversationProfileId, InteractionError, Participant, ProposalKind};

/// A validated interaction spec projected into runtime-facing manifests.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompiledInteractionSpec {
    id: InteractionSpecId,
    profiles: Vec<CompiledProfileManifest>,
}

impl CompiledInteractionSpec {
    /// Returns the interaction spec id.
    pub fn id(&self) -> &InteractionSpecId {
        &self.id
    }

    /// Returns compiled profile manifests in declaration order.
    pub fn profiles(&self) -> &[CompiledProfileManifest] {
        &self.profiles
    }

    /// Finds a compiled profile manifest by id.
    pub fn profile(&self, id: &ConversationProfileId) -> Option<&CompiledProfileManifest> {
        self.profiles
            .iter()
            .find(|profile| &profile.profile.id == id)
    }
}

/// Everything the runtime needs for one interactive profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompiledProfileManifest {
    /// Profile identity, participants, and recent-turn policy.
    pub profile: ProfileManifest,
    /// Forge transcript policy for this profile.
    pub transcript: TranscriptManifest,
    /// Responder process contract required by this profile.
    pub responder: ResponderManifest,
    /// Proposal kinds this profile accepts from its responder.
    pub proposals: Vec<ProposalManifest>,
    /// Transport-facing command aliases and action mapping.
    pub commands: Vec<CommandManifest>,
    /// Explicit accepted-proposal actions authorized for this profile.
    pub acceptance_actions: Vec<AcceptanceManifest>,
}

impl CompiledProfileManifest {
    /// Finds a proposal manifest by kind id.
    pub fn proposal(&self, kind: &ProposalKind) -> Option<&ProposalManifest> {
        self.proposals
            .iter()
            .find(|proposal| &proposal.kind == kind)
    }

    /// Finds a command manifest by id.
    pub fn command(&self, id: &CommandId) -> Option<&CommandManifest> {
        self.commands.iter().find(|command| &command.id == id)
    }

    /// Finds an acceptance manifest by id.
    pub fn acceptance_action(&self, id: &AcceptanceActionId) -> Option<&AcceptanceManifest> {
        self.acceptance_actions
            .iter()
            .find(|action| &action.id == id)
    }
}

/// Profile identity, participants, and transcript view policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileManifest {
    /// Profile id passed to responder requests and transport snapshots.
    pub id: ConversationProfileId,
    /// Participant representation for human turns.
    pub human_participant: Participant,
    /// Participant representation for agent turns.
    pub agent_participant: Participant,
    /// Maximum number of recent turns supplied to the responder.
    pub recent_turn_limit: usize,
}

/// Durable transcript policy for a compiled profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TranscriptManifest {
    /// Transcript backing target.
    pub target: TranscriptTargetKind,
    /// Exact labels applied to transcript issues.
    pub labels: Vec<String>,
    /// Label policy used when resuming transcripts.
    pub label_policy: TranscriptLabelPolicy,
    /// Prefix used for newly-created transcript issue titles.
    pub title_prefix: String,
    /// Hidden marker namespace for transcript correlation.
    pub marker_namespace: String,
}

/// External responder declaration selected by a profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResponderManifest {
    /// Stable responder id.
    pub id: ResponderId,
    /// Protocol/version the runtime must bind.
    pub protocol: ResponderProtocol,
    /// Whether deployments must provide this responder before serving.
    pub required: bool,
}

/// Proposal kind and payload validation contract.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProposalManifest {
    /// Stable proposal kind id.
    pub kind: ProposalKind,
    /// Payload validator for proposals of this kind.
    pub payload_validator: ProposalPayloadValidator,
}

impl ProposalManifest {
    /// Validates a proposal payload against this manifest's contract.
    pub fn validate_payload(&self, proposal: &Proposal) -> Result<(), InteractionError> {
        if proposal.kind != self.kind {
            return Err(InteractionError::UnsupportedProposalKind {
                id: proposal.id.clone(),
                kind: proposal.kind.clone(),
            });
        }
        self.payload_validator.validate(proposal)
    }
}

/// Closed payload validators known to the compiled interaction runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProposalPayloadValidator {
    /// Payload must deserialize as an [`IssueProposal`].
    IssueDraft,
}

impl ProposalPayloadValidator {
    /// Validates one proposal payload.
    pub fn validate(&self, proposal: &Proposal) -> Result<(), InteractionError> {
        match self {
            Self::IssueDraft => {
                let _: IssueProposal = serde_json::from_value(proposal.payload.clone())?;
                Ok(())
            }
        }
    }
}

/// Transport command exposed for a profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandManifest {
    /// Stable command id.
    pub id: CommandId,
    /// Transport aliases such as `/file`.
    pub aliases: Vec<String>,
    /// Runtime action requested by the command.
    pub action: CommandActionManifest,
}

/// Runtime action requested by a compiled command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommandActionManifest {
    /// Explicitly accept a proposal through an acceptance action.
    AcceptProposal {
        /// Proposal kind the command accepts.
        proposal_kind: ProposalKind,
        /// Acceptance action to execute.
        acceptance_action: AcceptanceActionId,
    },
}

/// Explicit accepted-proposal action manifest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AcceptanceManifest {
    /// Stable accepted-action id.
    pub id: AcceptanceActionId,
    /// Proposal kind this action accepts.
    pub proposal_kind: ProposalKind,
    /// Explicit acceptance policy.
    pub acceptance: AcceptancePolicy,
    /// Idempotency key template used by later acceptance execution.
    pub idempotency_key_template: String,
    /// Declared effects this action may apply after explicit acceptance.
    pub effects: Vec<AcceptanceEffect>,
}

/// Compiles a validated interaction spec into deterministic runtime manifests.
pub fn compile(spec: &ValidatedInteractionSpec) -> CompiledInteractionSpec {
    CompiledInteractionSpec {
        id: spec.id().clone(),
        profiles: spec
            .profiles()
            .iter()
            .map(|profile| compile_profile(spec, profile))
            .collect(),
    }
}

impl ValidatedInteractionSpec {
    /// Compiles this validated spec into runtime-facing manifests.
    pub fn compile(&self) -> CompiledInteractionSpec {
        compile(self)
    }
}

fn compile_profile(
    spec: &ValidatedInteractionSpec,
    profile: &ValidatedInteractiveProfile,
) -> CompiledProfileManifest {
    let responder = spec
        .responders()
        .iter()
        .find(|responder| responder.id() == profile.responder())
        .expect("validated profile responder reference should resolve");

    CompiledProfileManifest {
        profile: ProfileManifest {
            id: profile.id().clone(),
            human_participant: profile.participants().human().clone(),
            agent_participant: profile.participants().agent().clone(),
            recent_turn_limit: profile.transcript().recent_turn_limit(),
        },
        transcript: TranscriptManifest {
            target: profile.transcript().target(),
            labels: profile.transcript().labels().to_vec(),
            label_policy: profile.transcript().label_policy(),
            title_prefix: profile.transcript().title_prefix().to_string(),
            marker_namespace: profile.transcript().marker_namespace().to_string(),
        },
        responder: ResponderManifest {
            id: responder.id().clone(),
            protocol: responder.protocol(),
            required: responder.required(),
        },
        proposals: profile
            .proposal_kinds()
            .iter()
            .map(|proposal| ProposalManifest {
                kind: proposal.id().clone(),
                payload_validator: match proposal.payload() {
                    ProposalPayloadContract::IssueDraft => ProposalPayloadValidator::IssueDraft,
                },
            })
            .collect(),
        commands: profile
            .commands()
            .iter()
            .map(|command| CommandManifest {
                id: command.id().clone(),
                aliases: command.aliases().to_vec(),
                action: match command.action() {
                    TransportCommandAction::AcceptProposal {
                        kind,
                        acceptance_action,
                    } => CommandActionManifest::AcceptProposal {
                        proposal_kind: kind.clone(),
                        acceptance_action: acceptance_action.clone(),
                    },
                },
            })
            .collect(),
        acceptance_actions: profile
            .acceptance_actions()
            .iter()
            .map(|action| AcceptanceManifest {
                id: action.id().clone(),
                proposal_kind: action.proposal_kind().clone(),
                acceptance: action.acceptance().clone(),
                idempotency_key_template: action.idempotency_key_template().to_string(),
                effects: action.effects().to_vec(),
            })
            .collect(),
    }
}
