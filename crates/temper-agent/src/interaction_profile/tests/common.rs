//! Shared fixtures for the interaction-profile responder/config tests.

use std::path::Path;

use serde_json::json;
use temper_protocol_interaction::{
    ConversationId, ConversationProfileId, ConversationReply, ConversationRequest,
    ConversationTurn, Participant, Proposal, ProposalId, ProposalKind,
};

use crate::ProviderConfig;
use crate::interaction_profile::InteractionProfileConfig;

pub(super) fn dummy_provider() -> ProviderConfig {
    ProviderConfig::new(
        "test-provider",
        "test-model",
        "http://127.0.0.1",
        "dummy-secret",
    )
}

pub(super) fn support_profile_config() -> InteractionProfileConfig {
    let config_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/interaction-profiles/support-agent.json");
    InteractionProfileConfig::load_from_path(config_path)
        .expect("support profile fixture config validates")
}

pub(super) fn request_for(profile_id: &str) -> ConversationRequest {
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

pub(super) fn custom_support_reply() -> ConversationReply {
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
