//! `GenericInteractionResponder`: request/reply validation, prompt + context
//! rendering, prompt-safety, and the product-manager legacy-mapping fixture.

use std::path::Path;

use serde_json::json;
use temper_protocol_interaction::{
    ConversationReply, InteractionProtocolError, Proposal, ProposalId, ProposalKind,
};

use super::common::*;
use crate::interaction_profile::*;
use crate::{PRODUCT_MANAGER_SYSTEM_PROMPT, ProductManagerDraftIssue, ProductManagerResponse};

#[test]
fn non_product_profile_renders_context_and_validates_synthetic_reply() {
    let responder = GenericInteractionResponder::new(support_profile_config(), dummy_provider());
    let request = request_for("support-agent");
    let reply = custom_support_reply();

    responder
        .validate_request(&request)
        .expect("synthetic request validates");
    responder
        .validate_reply(&reply)
        .expect("synthetic reply validates");

    let system_prompt = responder.render_system_prompt();
    let user_context = responder
        .render_provider_context(&request)
        .expect("request context renders");
    assert!(system_prompt.contains("You help with support triage."));
    assert!(system_prompt.contains("Temper's ConversationReply v1"));
    assert!(user_context.contains("support-agent"));
    assert!(user_context.contains("conversation-1"));
    assert!(user_context.contains("The import flow keeps timing out."));
    assert!(user_context.contains("\"repository\": \"owner/repo\""));
    assert!(user_context.contains("support-escalation"));
}

#[test]
fn profile_id_mismatch_is_rejected_before_render_or_provider_call() {
    let responder = GenericInteractionResponder::new(support_profile_config(), dummy_provider());

    let error = responder
        .validate_request(&request_for("product-manager"))
        .expect_err("wrong profile is rejected");

    assert!(
        matches!(error, InteractionProfileError::InvalidRequest(message) if message.contains("product-manager"))
    );
}

#[test]
fn required_context_is_validated_before_render_or_provider_call() {
    let responder = GenericInteractionResponder::new(support_profile_config(), dummy_provider());
    let mut request = request_for("support-agent");
    request.context = json!({ "transcript_url": "https://example.test/transcript" });

    let error = responder
        .validate_request(&request)
        .expect_err("missing required context is rejected");

    assert!(
        matches!(error, InteractionProfileError::InvalidRequest(message) if message.contains("repository"))
    );
}

#[test]
fn proposal_kind_allow_list_rejects_undeclared_kind() {
    let reply = ConversationReply {
        message: "Try an issue instead.".into(),
        proposals: vec![Proposal::custom(
            ProposalId::new("unknown-proposal").expect("valid proposal"),
            ProposalKind::new("other-kind").expect("valid kind"),
            "Unknown proposal".to_string(),
            None,
            json!({ "value": true }),
        )],
    };
    let responder = GenericInteractionResponder::new(support_profile_config(), dummy_provider());

    let error = responder
        .validate_reply(&reply)
        .expect_err("undeclared proposal kind is rejected");

    assert!(matches!(
        error,
        InteractionProfileError::Protocol(InteractionProtocolError::UnsupportedProposalKind { kind, .. })
            if kind.as_str() == "other-kind"
    ));
}

#[test]
fn duplicate_proposal_ids_are_rejected_with_temper_validation() {
    let proposal = Proposal::custom(
        ProposalId::new("same-proposal").expect("valid proposal"),
        ProposalKind::new("support-escalation").expect("valid kind"),
        "Escalate".to_string(),
        None,
        json!({}),
    );
    let reply = ConversationReply {
        message: "duplicate".into(),
        proposals: vec![proposal.clone(), proposal],
    };
    let responder = GenericInteractionResponder::new(support_profile_config(), dummy_provider());

    let error = responder
        .validate_reply(&reply)
        .expect_err("duplicate ids fail");
    assert!(matches!(
        error,
        InteractionProfileError::Protocol(InteractionProtocolError::DuplicateProposalId { .. })
    ));
}

#[test]
fn product_manager_fixture_config_matches_existing_mapper_shape() {
    let config_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/interaction-profiles/product-manager.json");
    let config = InteractionProfileConfig::load_from_path(config_path)
        .expect("product-manager anvil profile fixture loads");
    assert_eq!(config.profile_id().as_str(), "product-manager");
    assert_eq!(config.system_prompt(), PRODUCT_MANAGER_SYSTEM_PROMPT);
    assert_eq!(config.required_context(), &["repository".to_string()]);
    assert_eq!(config.allowed_proposal_kinds().len(), 1);
    assert_eq!(config.allowed_proposal_kinds()[0].id, ProposalKind::issue());
    assert_eq!(
        config.allowed_proposal_kinds()[0].payload,
        InteractionProposalPayloadContract::IssueDraft
    );

    let legacy = ProductManagerResponse {
        reply: "File one small issue.".into(),
        drafts: vec![ProductManagerDraftIssue {
            slug: "mobile-chat-loop".into(),
            title: "Add mobile chat loop".into(),
            body: "Expose chat from a phone-friendly client.".into(),
            rationale: Some("Dogfood from mobile.".into()),
        }],
    };
    let expected = legacy
        .to_conversation_reply()
        .expect("legacy mapper produces a ConversationReply");
    let responder = GenericInteractionResponder::new(config, dummy_provider());

    responder
        .validate_request(&request_for("product-manager"))
        .expect("product-manager request validates");
    responder
        .validate_reply(&expected)
        .expect("fixture accepts legacy-mapped reply");

    let issue = expected.proposals[0]
        .issue_payload()
        .expect("issue payload decodes")
        .expect("issue payload is present");
    assert_eq!(issue.title, "Add mobile chat loop");
    assert_eq!(issue.body, "Expose chat from a phone-friendly client.");
    assert_eq!(issue.rationale.as_deref(), Some("Dogfood from mobile."));
}

#[test]
fn rendered_prompts_do_not_expose_provider_or_workflow_authority() {
    let responder = GenericInteractionResponder::new(support_profile_config(), dummy_provider());
    let request = request_for("support-agent");
    let rendered = format!(
        "{}\n{}",
        responder.render_system_prompt(),
        responder
            .render_provider_context(&request)
            .expect("request context renders")
    )
    .to_lowercase();

    for forbidden in [
        "dummy-secret",
        "forge",
        "token",
        "workflow",
        "bash",
        "tool",
        "provider key",
    ] {
        assert!(
            !rendered.contains(forbidden),
            "rendered prompt should not contain {forbidden:?}: {rendered}"
        );
    }
}
