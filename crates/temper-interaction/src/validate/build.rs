use crate::ids::{AcceptanceActionId, CommandId, InteractionSpecId, ResponderId};
use crate::spec::{
    RawAcceptanceActionDeclaration, RawAcceptanceEffect, RawInteractionSpec, RawInteractiveProfile,
    RawParticipantDeclaration, RawTranscriptPolicy, RawTransportCommandDeclaration,
};
use crate::types::{ConversationProfileId, ParticipantKind};
use crate::validated::{
    AcceptanceEffect, AcceptancePolicy, AddTranscriptCommentEffect, BacklinkPolicy,
    CreateIssueEffect, ProposalPayloadContract, ResponderProtocol, TranscriptLabelPolicy,
    TranscriptTargetKind, TransportCommandAction, ValidatedAcceptanceActionDeclaration,
    ValidatedInteractionSpec, ValidatedInteractiveProfile, ValidatedParticipants,
    ValidatedProposalKindDeclaration, ValidatedResponderDeclaration, ValidatedTranscriptPolicy,
    ValidatedTransportCommandDeclaration,
};
use crate::{Participant, ProposalKind};

const EFFECT_CREATE_ISSUE: &str = "create_issue";
const EFFECT_ADD_TRANSCRIPT_COMMENT: &str = "add_transcript_comment";

pub(super) fn build_validated(spec: &RawInteractionSpec) -> ValidatedInteractionSpec {
    ValidatedInteractionSpec {
        id: InteractionSpecId::new(&spec.id),
        responders: spec
            .responders
            .iter()
            .map(|responder| ValidatedResponderDeclaration {
                id: ResponderId::new(&responder.id),
                protocol: ResponderProtocol::ProcessV1,
                required: responder.required,
            })
            .collect(),
        profiles: spec.profiles.iter().map(build_profile).collect(),
    }
}

fn build_profile(profile: &RawInteractiveProfile) -> ValidatedInteractiveProfile {
    ValidatedInteractiveProfile {
        id: ConversationProfileId::new(&profile.id)
            .expect("validated profile id should be a deterministic slug"),
        transcript: build_transcript(&profile.transcript),
        participants: ValidatedParticipants {
            human: build_participant(ParticipantKind::Human, &profile.participants.human),
            agent: build_participant(ParticipantKind::Agent, &profile.participants.agent),
        },
        responder: ResponderId::new(&profile.responder),
        proposal_kinds: profile
            .proposal_kinds
            .iter()
            .map(|proposal_kind| ValidatedProposalKindDeclaration {
                id: ProposalKind::new(&proposal_kind.id)
                    .expect("validated proposal kind should be a deterministic slug"),
                payload: ProposalPayloadContract::IssueDraft,
            })
            .collect(),
        commands: profile.commands.iter().map(build_command).collect(),
        acceptance_actions: profile
            .acceptance_actions
            .iter()
            .map(build_acceptance_action)
            .collect(),
    }
}

fn build_transcript(transcript: &RawTranscriptPolicy) -> ValidatedTranscriptPolicy {
    ValidatedTranscriptPolicy {
        target: TranscriptTargetKind::ForgeIssue,
        title_prefix: transcript.title_prefix.trim().to_string(),
        labels: trim_vec(&transcript.labels),
        label_policy: TranscriptLabelPolicy::Exact,
        marker_namespace: transcript.marker_namespace.trim().to_string(),
        recent_turn_limit: transcript.recent_turn_limit,
    }
}

fn build_participant(kind: ParticipantKind, raw: &RawParticipantDeclaration) -> Participant {
    let display_name = raw.display_name.trim();
    Participant {
        kind,
        display_name: (!display_name.is_empty()).then(|| display_name.to_string()),
    }
}

fn build_command(command: &RawTransportCommandDeclaration) -> ValidatedTransportCommandDeclaration {
    let action = &command.action.accept_proposal;
    ValidatedTransportCommandDeclaration {
        id: CommandId::new(&command.id),
        aliases: command
            .aliases
            .iter()
            .map(|alias| alias.trim().to_string())
            .collect(),
        action: TransportCommandAction::AcceptProposal {
            kind: ProposalKind::new(&action.kind)
                .expect("validated command proposal kind should be a deterministic slug"),
            acceptance_action: AcceptanceActionId::new(&action.acceptance_action),
        },
    }
}

fn build_acceptance_action(
    action: &RawAcceptanceActionDeclaration,
) -> ValidatedAcceptanceActionDeclaration {
    ValidatedAcceptanceActionDeclaration {
        id: AcceptanceActionId::new(&action.id),
        proposal_kind: ProposalKind::new(&action.proposal_kind)
            .expect("validated acceptance proposal kind should be a deterministic slug"),
        acceptance: AcceptancePolicy::Explicit {
            commands: action
                .acceptance
                .commands
                .iter()
                .map(CommandId::new)
                .collect(),
        },
        idempotency_key_template: action.idempotency_key.trim().to_string(),
        effects: action
            .effects
            .iter()
            .map(|effect| build_effect(action, effect))
            .collect(),
    }
}

fn build_effect(
    action: &RawAcceptanceActionDeclaration,
    effect: &RawAcceptanceEffect,
) -> AcceptanceEffect {
    match effect.kind.as_str() {
        EFFECT_CREATE_ISSUE => AcceptanceEffect::CreateIssue(CreateIssueEffect {
            title: effect.title.trim().to_string(),
            body_template: effect.body_template.trim().to_string(),
            labels: trim_vec(&effect.labels),
            assignees: trim_vec(&effect.assignees),
            marker_namespace: effect.marker_namespace.trim().to_string(),
            marker_key: marker_key(action, effect),
            backlink: effect.backlink.as_ref().map(|backlink| BacklinkPolicy {
                label: backlink.label.trim().to_string(),
                url: backlink.url.trim().to_string(),
            }),
        }),
        EFFECT_ADD_TRANSCRIPT_COMMENT => {
            AcceptanceEffect::AddTranscriptComment(AddTranscriptCommentEffect {
                body_template: effect.body_template.trim().to_string(),
                marker_namespace: effect.marker_namespace.trim().to_string(),
                marker_key: marker_key(action, effect),
            })
        }
        _ => unreachable!("validated effects should be closed"),
    }
}

fn marker_key(
    action: &RawAcceptanceActionDeclaration,
    effect: &RawAcceptanceEffect,
) -> Option<String> {
    let value = effect.marker_key.trim();
    (!value.is_empty()).then(|| value.to_string()).or_else(|| {
        let action_id = action.id.trim();
        (!action_id.is_empty()).then(|| action_id.to_string())
    })
}

fn trim_vec(values: &[String]) -> Vec<String> {
    values
        .iter()
        .map(|value| value.trim().to_string())
        .collect()
}
