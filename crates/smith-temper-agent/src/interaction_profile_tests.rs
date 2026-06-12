use std::path::Path;

use serde_json::json;
use temper_process_protocol::{
    ConversationId, ConversationProfileId, ConversationReply, ConversationRequest,
    ConversationTurn, InteractionProtocolError, IssueProposal, Participant, Proposal, ProposalId,
    ProposalKind,
};
use uuid::Uuid;

use super::*;
use crate::{
    PRODUCT_MANAGER_SYSTEM_PROMPT, ProductManagerDraftIssue, ProductManagerResponse, ProviderConfig,
};

fn dummy_provider() -> ProviderConfig {
    ProviderConfig::new(
        "test-provider",
        "test-model",
        "http://127.0.0.1",
        "dummy-secret",
    )
}

fn support_profile_config() -> InteractionProfileConfig {
    let config_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/interaction-profiles/support-agent.json");
    InteractionProfileConfig::load_from_path(config_path)
        .expect("support profile fixture config validates")
}

fn request_for(profile_id: &str) -> ConversationRequest {
    ConversationRequest {
        profile_id: ConversationProfileId::new(profile_id).expect("valid profile"),
        conversation_id: ConversationId::new("conversation-1").expect("valid conversation"),
        turns: vec![ConversationTurn::new(
            Participant::human("human"),
            "The import flow keeps timing out.",
        )],
        context: json!({ "repository": "owner/repo" }),
    }
}

fn custom_support_reply() -> ConversationReply {
    ConversationReply {
        message: "I would escalate this with the import trace.".into(),
        proposals: vec![Proposal::custom(
            ProposalId::new("import-timeout-escalation").expect("valid proposal"),
            ProposalKind::new("support-escalation").expect("valid kind"),
            "Escalate import timeout".to_string(),
            Some("The user is blocked and has timing data.".to_string()),
            json!({ "priority": "high", "area": "imports" }),
        )],
    }
}

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
fn built_in_issue_payloads_are_rejected_with_temper_validation() {
    let config = InteractionProfileConfig::from_json_str(
        r#"{
          "profile_id": "issue-agent",
          "system_prompt": { "text": "Draft issues." },
          "allowed_proposal_kinds": [
            { "id": "issue", "payload": "issue_draft" }
          ],
          "response_format": "conversation_reply_v1"
        }"#,
    )
    .expect("config validates");
    let reply = ConversationReply {
        message: "broken issue".into(),
        proposals: vec![Proposal::custom(
            ProposalId::new("broken-issue").expect("valid proposal"),
            ProposalKind::issue(),
            "Broken issue".to_string(),
            None,
            json!({ "title": "Missing body" }),
        )],
    };
    let responder = GenericInteractionResponder::new(config, dummy_provider());

    let error = responder
        .validate_reply(&reply)
        .expect_err("invalid issue payload fails");
    assert!(matches!(
        error,
        InteractionProfileError::Protocol(InteractionProtocolError::Json(_))
    ));
}

#[test]
fn product_manager_fixture_config_matches_existing_mapper_shape() {
    let config_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/interaction-profiles/product-manager.json");
    let config = InteractionProfileConfig::load_from_path(config_path)
        .expect("product-manager Smith profile fixture loads");
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

#[test]
fn config_denies_unknown_fields() {
    let error = InteractionProfileConfig::from_json_str(
        r#"{
          "profile_id": "support-agent",
          "system_prompt": { "text": "Support users." },
          "provider_token_env": "SHOULD_NOT_EXIST",
          "response_format": "conversation_reply_v1"
        }"#,
    )
    .expect_err("unknown fields are denied");

    assert!(error.to_string().contains("unknown field"));
}

#[test]
fn config_requires_exactly_one_prompt_source() {
    let error = InteractionProfileConfig::from_json_str(
        r#"{
          "profile_id": "support-agent",
          "system_prompt": { "text": "Support users.", "path": "prompt.md" },
          "response_format": "conversation_reply_v1"
        }"#,
    )
    .expect_err("two prompt sources fail");

    assert!(matches!(
        error,
        InteractionProfileError::InvalidConfig {
            field: "system_prompt",
            ..
        }
    ));
}

#[test]
fn config_resolves_prompt_path_relative_to_config_file() {
    let root =
        std::env::temp_dir().join(format!("smith-interaction-profile-test-{}", Uuid::new_v4()));
    let prompt_dir = root.join("prompts");
    std::fs::create_dir_all(&prompt_dir).expect("create prompt dir");
    std::fs::write(prompt_dir.join("support.md"), "Path-loaded support prompt.")
        .expect("write prompt");
    let config_path = root.join("support.json");
    std::fs::write(
        &config_path,
        r#"{
          "profile_id": "support-agent",
          "system_prompt": { "path": "prompts/support.md" },
          "required_context": ["repository"],
          "response_format": "conversation_reply_v1"
        }"#,
    )
    .expect("write config");

    let config = InteractionProfileConfig::load_from_path(&config_path)
        .expect("relative prompt path resolves");

    assert_eq!(config.system_prompt(), "Path-loaded support prompt.");
    std::fs::remove_dir_all(root).expect("remove temp dir");
}

#[test]
fn config_rejects_duplicate_or_invalid_context_fields_and_proposal_kinds() {
    let duplicate_context = InteractionProfileConfig::from_json_str(
        r#"{
          "profile_id": "support-agent",
          "system_prompt": { "text": "Support users." },
          "required_context": ["repository", "repository"],
          "response_format": "conversation_reply_v1"
        }"#,
    )
    .expect_err("duplicate context fails");
    assert!(duplicate_context.to_string().contains("duplicate context"));

    let invalid_context = InteractionProfileConfig::from_json_str(
        r#"{
          "profile_id": "support-agent",
          "system_prompt": { "text": "Support users." },
          "required_context": ["bad field"],
          "response_format": "conversation_reply_v1"
        }"#,
    )
    .expect_err("invalid context fails");
    assert!(
        invalid_context
            .to_string()
            .contains("invalid context field")
    );

    let duplicate_kind = InteractionProfileConfig::from_json_str(
        r#"{
          "profile_id": "support-agent",
          "system_prompt": { "text": "Support users." },
          "allowed_proposal_kinds": [
            { "id": "support-escalation", "payload": "custom_json" },
            { "id": "support-escalation", "payload": "custom_json" }
          ],
          "response_format": "conversation_reply_v1"
        }"#,
    )
    .expect_err("duplicate proposal kind fails");
    assert!(
        duplicate_kind
            .to_string()
            .contains("duplicate proposal kind")
    );

    let invalid_kind = InteractionProfileConfig::from_json_str(
        r#"{
          "profile_id": "support-agent",
          "system_prompt": { "text": "Support users." },
          "allowed_proposal_kinds": [
            { "id": "Support", "payload": "custom_json" }
          ],
          "response_format": "conversation_reply_v1"
        }"#,
    )
    .expect_err("invalid proposal kind fails");
    assert!(invalid_kind.to_string().contains("proposal kind"));

    let invalid_payload = InteractionProfileConfig::from_json_str(
        r#"{
          "profile_id": "support-agent",
          "system_prompt": { "text": "Support users." },
          "allowed_proposal_kinds": [
            { "id": "support-escalation", "payload": "issue_draft" }
          ],
          "response_format": "conversation_reply_v1"
        }"#,
    )
    .expect_err("issue_draft on custom kind fails");
    assert!(invalid_payload.to_string().contains("issue_draft"));
}

#[test]
fn config_rejects_invalid_profile_id() {
    let error = InteractionProfileConfig::from_json_str(
        r#"{
          "profile_id": "Support Agent",
          "system_prompt": { "text": "Support users." },
          "response_format": "conversation_reply_v1"
        }"#,
    )
    .expect_err("profile id must be deterministic");

    assert!(matches!(
        error,
        InteractionProfileError::InvalidConfig {
            field: "profile_id",
            ..
        }
    ));
}

#[test]
fn issue_proposal_helper_still_produces_a_reply_allowed_by_issue_config() {
    let config = InteractionProfileConfig::from_json_str(
        r#"{
          "profile_id": "issue-agent",
          "system_prompt": { "text": "Draft issues." },
          "allowed_proposal_kinds": [
            { "id": "issue", "payload": "issue_draft" }
          ],
          "response_format": "conversation_reply_v1"
        }"#,
    )
    .expect("config validates");
    let reply = ConversationReply {
        message: "Drafted one issue.".into(),
        proposals: vec![
            Proposal::issue(
                ProposalId::new("mobile-chat-loop").expect("valid proposal"),
                IssueProposal::with_rationale(
                    "Add mobile chat loop",
                    "Expose chat from a phone-friendly client.",
                    "Dogfood from mobile.",
                ),
            )
            .expect("issue proposal builds"),
        ],
    };
    let responder = GenericInteractionResponder::new(config, dummy_provider());

    responder
        .validate_reply(&reply)
        .expect("issue reply is allowed");
}
