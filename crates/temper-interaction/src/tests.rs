use async_trait::async_trait;
use serde_json::json;

use crate::{
    is_valid_proposal_slug, ConversationId, ConversationProfileId, ConversationReply,
    ConversationRequest, ConversationTurn, InteractionError, InteractiveResponder, IssueProposal,
    Participant, Proposal, ProposalId, ProposalKind,
};

fn proposal_id(value: &str) -> ProposalId {
    ProposalId::new(value).expect("valid proposal id")
}

#[test]
fn process_boundary_request_and_reply_json_round_trip() {
    let request_json = r#"{
        "profile_id": "intake-assistant",
        "conversation_id": "conversation-1",
        "turns": [{
            "id": "turn-1",
            "participant": { "kind": "human", "display_name": "human" },
            "body": "Can we add a mobile chat adapter?"
        }],
        "context": {
            "repository": "owner/repo",
            "transcript_url": "https://git.example.test/owner/repo/issues/7"
        }
    }"#;
    let request: ConversationRequest =
        serde_json::from_str(request_json).expect("process request deserializes");
    assert_eq!(request.profile_id.as_str(), "intake-assistant");
    assert_eq!(request.conversation_id.as_str(), "conversation-1");
    assert_eq!(request.turns[0].id.as_ref().unwrap().as_str(), "turn-1");

    let encoded_request = serde_json::to_string(&request).expect("request serializes");
    let decoded_request: ConversationRequest =
        serde_json::from_str(&encoded_request).expect("request round-trips");
    assert_eq!(decoded_request, request);

    let reply_json = r#"{
        "message": "I would file one issue for the adapter first.",
        "proposals": [{
            "id": "mobile-chat-adapter",
            "kind": "issue",
            "title": "Add mobile chat adapter",
            "summary": "Mobile access lets humans keep the conversation moving.",
            "payload": {
                "title": "Add mobile chat adapter",
                "body": "Expose the interaction service through a mobile-friendly adapter.",
                "rationale": "Mobile access lets humans keep the conversation moving."
            }
        }]
    }"#;
    let reply: ConversationReply =
        serde_json::from_str(reply_json).expect("process reply deserializes");
    reply.validate().expect("reply validates");

    let issue = IssueProposal::with_rationale(
        "Add mobile chat adapter",
        "Expose the interaction service through a mobile-friendly adapter.",
        "Mobile access lets humans keep the conversation moving.",
    );
    assert_eq!(
        reply.proposals[0]
            .issue_payload()
            .expect("issue payload decodes"),
        Some(issue)
    );

    let encoded_reply = serde_json::to_string_pretty(&reply).expect("reply serializes");
    assert!(encoded_reply.contains("mobile-chat-adapter"));
    assert!(encoded_reply.contains("issue"));
    let decoded_reply: ConversationReply =
        serde_json::from_str(&encoded_reply).expect("reply round-trips");
    assert_eq!(decoded_reply, reply);
}

#[test]
fn validates_deterministic_proposal_slugs() {
    for slug in ["mvp", "matrix-text-adapter", "api-v1", "a1-b2"] {
        assert!(is_valid_proposal_slug(slug), "{slug} should be valid");
        ProposalId::new(slug).expect("valid proposal id");
        ProposalKind::new(slug).expect("valid proposal kind");
    }

    for slug in [
        "",
        "Matrix",
        "matrix_text",
        "matrix--text",
        "-matrix",
        "matrix-",
        "matrix text",
        "mátřix",
    ] {
        assert!(!is_valid_proposal_slug(slug), "{slug} should be invalid");
        assert!(matches!(
            ProposalId::new(slug),
            Err(InteractionError::InvalidSlug { .. })
        ));
    }

    let too_long = "a".repeat(81);
    assert!(!is_valid_proposal_slug(&too_long));
}

#[test]
fn rejects_invalid_ids_during_deserialization() {
    let json = r#"{
        "message": "bad id",
        "proposals": [{
            "id": "bad_id",
            "kind": "issue",
            "title": "Bad id",
            "summary": null,
            "payload": {}
        }]
    }"#;

    let error = serde_json::from_str::<ConversationReply>(json).expect_err("invalid id fails");
    assert!(error.to_string().contains("invalid proposal id"));
}

#[test]
fn rejects_duplicate_proposal_ids() {
    let first = Proposal::custom(
        proposal_id("same-proposal"),
        ProposalKind::issue(),
        "First",
        None,
        json!({}),
    );
    let second = Proposal::custom(
        proposal_id("same-proposal"),
        ProposalKind::new("custom-kind").expect("valid kind"),
        "Second",
        None,
        json!({}),
    );
    let reply = ConversationReply {
        message: "duplicate".to_string(),
        proposals: vec![first, second],
    };

    assert!(matches!(
        reply.validate(),
        Err(InteractionError::DuplicateProposalId { .. })
    ));
}

struct EchoResponder;

#[async_trait]
impl InteractiveResponder for EchoResponder {
    async fn respond(
        &self,
        request: &ConversationRequest,
    ) -> Result<ConversationReply, InteractionError> {
        Ok(ConversationReply::message(format!(
            "{}:{} turn(s)",
            request.profile_id,
            request.turns.len()
        )))
    }
}

fn accepts_trait_object(_responder: &dyn InteractiveResponder) {}

#[test]
fn interactive_responder_is_object_safe() {
    let responder: Box<dyn InteractiveResponder> = Box::new(EchoResponder);
    accepts_trait_object(responder.as_ref());

    let request = ConversationRequest::new(
        ConversationProfileId::new("echo-profile").expect("valid profile"),
        ConversationId::new("conversation-1").expect("valid conversation"),
        vec![ConversationTurn::new(Participant::human("human"), "hello")],
    );
    let future = responder.respond(&request);
    drop(future);
}
