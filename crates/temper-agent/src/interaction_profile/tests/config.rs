//! `InteractionProfileConfig` JSON validation: unknown-field denial, prompt
//! source rules, context/proposal-kind validation, and payload contracts.

use serde_json::json;
use temper_protocol_interaction::{
    ConversationReply, InteractionProtocolError, IssueProposal, Proposal, ProposalId, ProposalKind,
};
use uuid::Uuid;

use super::common::*;
use crate::interaction_profile::*;

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
        std::env::temp_dir().join(format!("anvil-interaction-profile-test-{}", Uuid::new_v4()));
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
