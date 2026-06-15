//! Builders that turn a live session into profile-neutral API responses.

use serde_json::{Map, Value};
use temper_forge::{Forge, Issue};
use temper_interaction::{
    AcceptedProposalTarget, CompiledProfileManifest, ConversationTranscriptRef, ForgeInteractionSession,
    InteractiveResponder, ProposalId,
};

use crate::interaction_service::{
    AcceptProposalResponse, ConversationResponse, TranscriptResponse,
};

pub(super) type DynSession =
    ForgeInteractionSession<dyn Forge, dyn Forge, dyn InteractiveResponder>;

pub(super) fn conversation_response(
    session: &DynSession,
    profile: &CompiledProfileManifest,
) -> ConversationResponse {
    ConversationResponse {
        id: session.conversation_id().to_string(),
        profile_id: profile.profile.id.to_string(),
        transcript: transcript_response(session),
        turns: session.turns().to_vec(),
        latest_proposals: session.latest_proposals().to_vec(),
    }
}

pub(super) fn transcript_response(session: &DynSession) -> TranscriptResponse {
    TranscriptResponse {
        issue_number: session.transcript_issue().number.get(),
        url: session.transcript_url(),
    }
}

pub(super) fn transcript_ref(response: &TranscriptResponse) -> ConversationTranscriptRef {
    ConversationTranscriptRef::forge_issue(response.issue_number, response.url.clone())
}

pub(super) fn accept_response(
    session: &DynSession,
    proposal_id: ProposalId,
    created: bool,
    issue: &Issue,
) -> AcceptProposalResponse {
    AcceptProposalResponse {
        proposal_id,
        created,
        target: Some(AcceptedProposalTarget::issue(
            issue.number.get(),
            session.issue_url_for(issue.number),
            issue.title.clone(),
        )),
        transcript: transcript_response(session),
    }
}

pub(super) fn normalize_context(context: Value) -> Value {
    if context.is_null() {
        Value::Object(Map::new())
    } else {
        context
    }
}
