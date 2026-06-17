use super::*;
use temper_protocol_interaction::{
    ConversationId, ConversationProfileId, ConversationTurn, Participant, ProposalKind,
};

#[test]
fn product_manager_parses_json_response_with_zero_drafts() {
    let response: ProductManagerResponse = serde_json::from_str(
        r#"{
          "reply": "Let's first clarify the mobile use case.",
          "drafts": []
        }"#,
    )
    .expect("response parses");

    response.validate().expect("drafts validate");
    assert_eq!(response.reply, "Let's first clarify the mobile use case.");
    assert!(response.drafts.is_empty());
}

#[test]
fn product_manager_parses_json_response_with_multiple_drafts() {
    let response: ProductManagerResponse = serde_json::from_str(
        r#"{
          "reply": "I would split this into two cheap dogfood steps.",
          "drafts": [
            {
              "slug": "matrix-text-adapter",
              "title": "Add Matrix text adapter for product-manager chat",
              "body": "Create a Matrix text bridge so Android users can dogfood product-manager chat.",
              "rationale": "Matrix gives mobile access without building a custom app first."
            },
            {
              "slug": "local-chat-service-api",
              "title": "Expose product-manager chat through a local service API",
              "body": "Add a loopback API that external clients can call for transcript turns and drafts.",
              "rationale": null
            }
          ]
        }"#,
    )
    .expect("response parses");

    response.validate().expect("drafts validate");
    assert_eq!(response.drafts.len(), 2);
    assert_eq!(response.drafts[0].slug, "matrix-text-adapter");
    assert_eq!(response.drafts[1].rationale, None);
}

#[test]
fn product_manager_maps_generic_interaction_request_and_reply() {
    let request = ConversationRequest {
        profile_id: ConversationProfileId::new(PRODUCT_MANAGER_PROFILE_ID).expect("valid profile"),
        conversation_id: ConversationId::new("conversation-1").expect("valid conversation"),
        turns: vec![
            ConversationTurn::new(Participant::human("human"), "I want a mobile chat loop."),
            ConversationTurn::new(
                Participant::agent("product-manager"),
                "Let's keep it small.",
            ),
            ConversationTurn::new(
                Participant::new(ParticipantKind::System),
                "ignored runtime note",
            ),
        ],
        context: serde_json::json!({
            "repository": "ai/temper",
            "transcript_url": "https://git.example.test/ai/temper/issues/1"
        }),
    };

    let mapped = ProductManagerRequest::from_conversation_request(&request).unwrap();
    assert_eq!(mapped.repository, "ai/temper");
    assert_eq!(mapped.turns.len(), 2);
    assert_eq!(mapped.turns[0].author, ProductManagerAuthor::Human);
    assert_eq!(mapped.turns[1].author, ProductManagerAuthor::ProductManager);

    let response = ProductManagerResponse {
        reply: "File one small issue.".into(),
        drafts: vec![ProductManagerDraftIssue {
            slug: "mobile-chat-loop".into(),
            title: "Add mobile chat loop".into(),
            body: "Expose chat from a phone-friendly client.".into(),
            rationale: Some("Dogfood from mobile.".into()),
        }],
    };
    let reply = response.to_conversation_reply().unwrap();
    reply.validate().expect("reply proposals validate");
    assert_eq!(reply.message, "File one small issue.");
    assert_eq!(reply.proposals[0].id.as_str(), "mobile-chat-loop");
    assert_eq!(reply.proposals[0].kind, ProposalKind::issue());
    assert_eq!(reply.proposals[0].title, "Add mobile chat loop");
    assert_eq!(
        reply.proposals[0].summary.as_deref(),
        Some("Dogfood from mobile.")
    );
    let issue = reply.proposals[0]
        .issue_payload()
        .expect("issue payload decodes")
        .expect("issue payload is present");
    assert_eq!(issue.title, "Add mobile chat loop");
    assert_eq!(issue.body, "Expose chat from a phone-friendly client.");
    assert_eq!(issue.rationale.as_deref(), Some("Dogfood from mobile."));
}

#[test]
fn product_manager_reads_temper_process_request_fixture() {
    let fixture = include_str!(
        "../../temper-protocol-interaction/fixtures/interactive-responder-request.json"
    );
    let request: ConversationRequest = serde_json::from_str(fixture).expect("fixture parses");
    let mapped = ProductManagerRequest::from_conversation_request(&request).unwrap();

    assert_eq!(request.profile_id.as_str(), PRODUCT_MANAGER_PROFILE_ID);
    assert_eq!(mapped.repository, "owner/repo");
    assert_eq!(mapped.turns.len(), 1);
    assert_eq!(mapped.turns[0].author, ProductManagerAuthor::Human);
}

#[test]
fn product_manager_reads_temper_process_reply_fixture_and_issue_payload_contract() {
    let fixture =
        include_str!("../../temper-protocol-interaction/fixtures/interactive-responder-reply.json");
    let reply: ConversationReply = serde_json::from_str(fixture).expect("fixture parses");
    reply.validate().expect("fixture reply validates");

    assert_eq!(
        reply.message,
        "I would file one issue for the adapter first."
    );
    assert_eq!(reply.proposals.len(), 1);
    let proposal = &reply.proposals[0];
    assert_eq!(proposal.id.as_str(), "mobile-chat-adapter");
    assert_eq!(proposal.kind, ProposalKind::issue());
    assert_eq!(proposal.title, "Add mobile chat adapter");
    assert_eq!(
        proposal.summary.as_deref(),
        Some("Mobile access lets humans keep the conversation moving.")
    );
    let issue = proposal
        .issue_payload()
        .expect("issue payload decodes")
        .expect("issue payload is present");
    assert_eq!(issue.title, "Add mobile chat adapter");
    assert_eq!(
        issue.body,
        "Expose the interaction service through a mobile-friendly adapter."
    );
    assert_eq!(
        issue.rationale.as_deref(),
        Some("Mobile access lets humans keep the conversation moving.")
    );
}

// NOTE: a former test here loaded temper-interaction's
// `product-manager-interaction-spec.json` fixture via `RawInteractionSpec`
// to cross-check anvil's profile/proposal assumptions against temper's
// compiled interaction manifest. It was removed when anvil dropped its
// dependency on `temper-interaction`: that crate is no longer serde-only
// (it pulls `temper-forge` + `temper-engine-io`), and anvil depends on
// temper only through the pure serde DTO crates. anvil's own
// ConversationRequest/Reply parsing stays covered by the
// `temper-protocol-interaction` fixture tests above.

#[test]
fn product_manager_responder_rejects_other_profiles_without_provider_call() {
    temper_agent_io::block_on_with(|_cx, handle| async move {
        rejects_other_profiles_without_provider_call_inner(handle).await;
    });
}

async fn rejects_other_profiles_without_provider_call_inner(handle: skein::runtime::RuntimeHandle) {
    let responder = ProductManagerResponder::new(ProviderConfig::new(
        "test-provider",
        "test-model",
        "http://127.0.0.1",
        "dummy-api-key",
    ));
    let request = ConversationRequest::new(
        ConversationProfileId::new("support-agent").expect("valid profile"),
        ConversationId::new("conversation-1").expect("valid conversation"),
        Vec::new(),
    );

    let error = responder
        .respond(handle, &request)
        .await
        .expect_err("non-product profile is rejected before provider call");
    assert!(matches!(
        error,
        ProductManagerError::InvalidRequest(message) if message.contains("support-agent")
    ));
}

#[test]
fn product_manager_validates_draft_slugs() {
    for slug in ["mvp", "matrix-text-adapter", "api-v1", "a1-b2"] {
        assert!(is_valid_draft_slug(slug), "{slug} should be valid");
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
        assert!(!is_valid_draft_slug(slug), "{slug} should be invalid");
    }

    let response = ProductManagerResponse {
        reply: "draft".to_string(),
        drafts: vec![ProductManagerDraftIssue {
            slug: "bad_slug".to_string(),
            title: "Bad slug".to_string(),
            body: "Body".to_string(),
            rationale: None,
        }],
    };
    assert!(matches!(
        response.validate(),
        Err(ProductManagerError::InvalidDraftSlug { .. })
    ));
}

#[test]
fn product_manager_rejects_duplicate_draft_slugs() {
    let draft = ProductManagerDraftIssue {
        slug: "same-draft".to_string(),
        title: "Draft".to_string(),
        body: "Body".to_string(),
        rationale: None,
    };
    let response = ProductManagerResponse {
        reply: "drafts".to_string(),
        drafts: vec![draft.clone(), draft],
    };

    assert!(matches!(
        response.validate(),
        Err(ProductManagerError::DuplicateDraftSlug { .. })
    ));
}

#[test]
fn product_manager_prompt_export_is_wired() {
    assert!(PRODUCT_MANAGER_SYSTEM_PROMPT.contains("product-manager"));
    assert!(PRODUCT_MANAGER_SYSTEM_PROMPT.contains("exactly one"));
    assert!(PRODUCT_MANAGER_SYSTEM_PROMPT.contains("stable"));
}
