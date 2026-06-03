use serde_json::json;

use crate::{
    compile, AcceptanceEffect, CommandActionManifest, ForgeSessionConfig, ForgeTranscriptConfig,
    Proposal, ProposalId, ProposalKind, ProposalPayloadValidator, RawInteractionSpec,
};

fn generic_spec() -> RawInteractionSpec {
    serde_json::from_value(json!({
        "id": "support-interactions",
        "responders": [{
            "id": "support-responder",
            "protocol": "process-v1",
            "required": true
        }],
        "profiles": [{
            "id": "support-agent",
            "transcript": {
                "target": "issue",
                "title_prefix": "Support conversation",
                "labels": ["support-transcript", "customer-visible"],
                "label_policy": "exact",
                "marker_namespace": "support-chat",
                "recent_turn_limit": 12
            },
            "participants": {
                "human": { "display_name": "customer" },
                "agent": { "display_name": "support-agent" }
            },
            "responder": "support-responder",
            "proposal_kinds": [{
                "id": "issue",
                "payload": "issue_draft"
            }],
            "commands": [{
                "id": "accept-issue",
                "aliases": ["/accept"],
                "action": {
                    "accept_proposal": {
                        "kind": "issue",
                        "acceptance_action": "accept-issue"
                    }
                }
            }],
            "acceptance_actions": [{
                "id": "accept-issue",
                "proposal_kind": "issue",
                "acceptance": {
                    "policy": "explicit",
                    "commands": ["accept-issue"]
                },
                "idempotency_key": "${conversation.id}:${proposal.id}",
                "effects": [{
                    "kind": "create_issue",
                    "title": "${proposal.payload.title}",
                    "body_template": "${proposal.payload.body}\n\nTranscript: ${conversation.transcript_url}",
                    "labels": ["support-intake", "needs-triage"],
                    "marker_namespace": "support-chat",
                    "backlink": {
                        "label": "Transcript",
                        "url": "${conversation.transcript_url}"
                    }
                }]
            }]
        }]
    }))
    .expect("generic spec shape is valid")
}

fn product_manager_fixture() -> RawInteractionSpec {
    serde_json::from_str(include_str!(
        "../fixtures/product-manager-interaction-spec.json"
    ))
    .expect("fixture deserializes")
}

#[test]
fn compilation_is_deterministic_and_structured() {
    let validated = generic_spec().validate().expect("validates");
    let first = compile(&validated);
    let second = validated.compile();
    assert_eq!(first, second);

    let profile = &first.profiles()[0];
    assert_eq!(first.id().as_str(), "support-interactions");
    assert_eq!(profile.profile.id.as_str(), "support-agent");
    assert_eq!(profile.profile.recent_turn_limit, 12);
    assert_eq!(
        profile.profile.human_participant.display_name.as_deref(),
        Some("customer")
    );
    assert_eq!(
        profile.transcript.labels,
        ["support-transcript", "customer-visible"]
    );
    assert_eq!(profile.transcript.title_prefix, "Support conversation");
    assert_eq!(profile.responder.id.as_str(), "support-responder");
    assert!(profile.responder.required);
    assert_eq!(
        profile.proposals[0].payload_validator,
        ProposalPayloadValidator::IssueDraft
    );
    assert_eq!(profile.commands[0].aliases, ["/accept"]);
    assert!(matches!(
        &profile.commands[0].action,
        CommandActionManifest::AcceptProposal { proposal_kind, acceptance_action }
            if proposal_kind.as_str() == "issue" && acceptance_action.as_str() == "accept-issue"
    ));
    let AcceptanceEffect::CreateIssue(effect) = &profile.acceptance_actions[0].effects[0];
    assert_eq!(effect.labels(), ["support-intake", "needs-triage"]);
}

#[test]
fn proposal_manifest_validates_payload_shape() {
    let compiled = generic_spec().validate().unwrap().compile();
    let manifest = compiled.profiles()[0]
        .proposal(&ProposalKind::issue())
        .expect("issue proposal manifest");
    let proposal = Proposal::issue(
        ProposalId::new("support-mvp").unwrap(),
        crate::IssueProposal::new("Support MVP", "Build the MVP."),
    )
    .unwrap();
    manifest
        .validate_payload(&proposal)
        .expect("issue draft payload validates");
}

#[test]
fn session_configs_can_be_built_from_arbitrary_profile_manifests() {
    let compiled = generic_spec().validate().unwrap().compile();
    let profile = &compiled.profiles()[0];

    let transcript = ForgeTranscriptConfig::from_profile_manifest(profile);
    assert_eq!(transcript.profile_id.as_str(), "support-agent");
    assert_eq!(
        transcript.transcript_labels,
        ["support-transcript", "customer-visible"]
    );
    assert_eq!(transcript.transcript_title_prefix, "Support conversation");
    assert_eq!(transcript.marker_namespace, "support-chat");
    assert_eq!(transcript.conversation_id_prefix, "support-agent");
    assert_eq!(transcript.recent_turn_limit, 12);

    let session = ForgeSessionConfig::from_profile_manifest(profile).unwrap();
    assert_eq!(session.transcript, transcript);
    assert_eq!(
        session.issue_intake.issue_labels,
        ["support-intake", "needs-triage"]
    );
    assert_eq!(session.issue_intake.marker_namespace, "support-chat");
}

#[test]
fn product_manager_fixture_compiles_to_manifest_data() {
    let compiled = product_manager_fixture().validate().unwrap().compile();
    let profile = &compiled.profiles()[0];

    assert_eq!(profile.profile.id.as_str(), "product-manager");
    assert_eq!(profile.transcript.labels, ["product"]);
    assert_eq!(profile.transcript.title_prefix, "Product conversation");
    assert_eq!(profile.transcript.marker_namespace, "product-chat");
    assert_eq!(profile.commands[0].aliases, ["/file"]);
    let AcceptanceEffect::CreateIssue(effect) = &profile.acceptance_actions[0].effects[0];
    assert_eq!(effect.labels(), ["untriaged"]);
}

#[test]
fn compiler_and_session_logic_have_no_product_manager_literal() {
    for (path, source) in [
        ("compile.rs", include_str!("compile.rs")),
        ("session.rs", include_str!("session.rs")),
        ("transcript.rs", include_str!("transcript.rs")),
    ] {
        assert!(
            !source.contains("product-manager"),
            "{path} should stay profile-neutral"
        );
    }
}
