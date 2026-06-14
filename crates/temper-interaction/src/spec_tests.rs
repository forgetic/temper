use serde_json::{Value, json};

use crate::{
    AcceptanceEffect, InteractionSpecDiagnostic, InteractionSpecReferenceSite,
    InteractionSpecSymbolKind, ProposalPayloadContract, RawInteractionSpec, TranscriptLabelPolicy,
    TranscriptTargetKind,
};

fn generic_spec(profile_id: &str) -> RawInteractionSpec {
    serde_json::from_value(json!({
        "id": "support-interactions",
        "responders": [{
            "id": "support-responder",
            "protocol": "process-v1",
            "required": true
        }],
        "profiles": [{
            "id": profile_id,
            "transcript": {
                "target": "issue",
                "title_prefix": "Support conversation",
                "labels": ["support-transcript"],
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
                "aliases": ["/accept", " accept "],
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
                    "labels": ["support-intake"],
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

fn has_diagnostic(
    error: &crate::InteractionSpecValidationErrors,
    expected: impl Fn(&InteractionSpecDiagnostic) -> bool,
) -> bool {
    error.diagnostics().iter().any(expected)
}

#[test]
fn generic_non_product_profile_validates() {
    let validated = generic_spec("support-agent")
        .validate()
        .expect("generic profile validates");

    assert_eq!(validated.id().as_str(), "support-interactions");
    assert_eq!(validated.responders()[0].id().as_str(), "support-responder");
    let profile = &validated.profiles()[0];
    assert_eq!(profile.id().as_str(), "support-agent");
    assert_eq!(profile.responder().as_str(), "support-responder");
    assert_eq!(
        profile.transcript().target(),
        TranscriptTargetKind::ForgeIssue
    );
    assert_eq!(
        profile.transcript().label_policy(),
        TranscriptLabelPolicy::Exact
    );
    assert_eq!(
        profile.transcript().labels(),
        &["support-transcript".to_string()]
    );
    assert_eq!(profile.transcript().recent_turn_limit(), 12);
    assert_eq!(
        profile.commands()[0].aliases(),
        &["/accept".to_string(), "accept".to_string()]
    );
    assert_eq!(
        profile.proposal_kinds()[0].payload(),
        ProposalPayloadContract::IssueDraft
    );
    let AcceptanceEffect::CreateIssue(effect) = &profile.acceptance_actions()[0].effects()[0]
    else {
        panic!("first generic effect creates an issue")
    };
    assert_eq!(effect.labels(), &["support-intake".to_string()]);
    assert_eq!(effect.backlink().unwrap().label(), "Transcript");
}

#[test]
fn product_manager_fixture_validates() {
    let validated = product_manager_fixture()
        .validate()
        .expect("product-manager fixture validates");
    let profile = &validated.profiles()[0];

    assert_eq!(validated.id().as_str(), "dogfood-interactions");
    assert_eq!(profile.id().as_str(), "product-manager");
    assert_eq!(profile.transcript().labels(), &["product".to_string()]);
    assert_eq!(profile.transcript().marker_namespace(), "product-chat");
    assert_eq!(profile.commands()[0].aliases(), &["/file".to_string()]);
    let AcceptanceEffect::CreateIssue(effect) = &profile.acceptance_actions()[0].effects()[0]
    else {
        panic!("product fixture first effect creates an issue")
    };
    assert_eq!(effect.labels(), &["untriaged".to_string()]);
}

#[test]
fn validation_collects_duplicate_ids() {
    let mut spec = generic_spec("support-agent");
    spec.responders.push(spec.responders[0].clone());
    let profile = &mut spec.profiles[0];
    profile
        .proposal_kinds
        .push(profile.proposal_kinds[0].clone());
    profile.commands.push(profile.commands[0].clone());
    profile
        .acceptance_actions
        .push(profile.acceptance_actions[0].clone());

    let error = spec.validate().expect_err("duplicates are rejected");
    assert!(has_diagnostic(&error, |diagnostic| matches!(
        diagnostic,
        InteractionSpecDiagnostic::DuplicateId {
            kind: InteractionSpecSymbolKind::Responder,
            id,
            ..
        } if id == "support-responder"
    )));
    assert!(has_diagnostic(&error, |diagnostic| matches!(
        diagnostic,
        InteractionSpecDiagnostic::DuplicateId {
            kind: InteractionSpecSymbolKind::Command,
            profile: Some(profile),
            id,
        } if profile == "support-agent" && id == "accept-issue"
    )));
}

#[test]
fn validation_catches_bad_slugs_and_empty_transcript_policy() {
    let mut spec = generic_spec("support-agent");
    spec.id = "Bad Spec".into();
    let profile = &mut spec.profiles[0];
    profile.id = "Support_Agent".into();
    profile.commands[0].id = "bad command".into();
    profile.transcript.labels = vec![" ".into()];
    profile.transcript.title_prefix.clear();
    profile.transcript.marker_namespace.clear();

    let error = spec
        .validate()
        .expect_err("invalid ids and transcript policy fail");
    assert!(has_diagnostic(&error, |diagnostic| matches!(
        diagnostic,
        InteractionSpecDiagnostic::InvalidSlug {
            kind: InteractionSpecSymbolKind::Spec,
            value,
            ..
        } if value == "Bad Spec"
    )));
    assert!(has_diagnostic(&error, |diagnostic| matches!(
        diagnostic,
        InteractionSpecDiagnostic::InvalidSlug {
            kind: InteractionSpecSymbolKind::Profile,
            value,
            ..
        } if value == "Support_Agent"
    )));
    assert!(has_diagnostic(&error, |diagnostic| matches!(
        diagnostic,
        InteractionSpecDiagnostic::EmptyTranscriptLabel { .. }
    )));
    assert!(has_diagnostic(&error, |diagnostic| matches!(
        diagnostic,
        InteractionSpecDiagnostic::EmptyTranscriptTitlePrefix { .. }
    )));
    assert!(has_diagnostic(&error, |diagnostic| matches!(
        diagnostic,
        InteractionSpecDiagnostic::EmptyTranscriptMarkerNamespace { .. }
    )));
}

#[test]
fn validation_catches_bad_references() {
    let mut spec = generic_spec("support-agent");
    let profile = &mut spec.profiles[0];
    profile.responder = "missing-responder".into();
    profile.commands[0].action.accept_proposal.kind = "missing-kind".into();
    profile.commands[0].action.accept_proposal.acceptance_action = "missing-action".into();
    profile.acceptance_actions[0].proposal_kind = "other-kind".into();
    profile.acceptance_actions[0].acceptance.commands = vec!["missing-command".into()];

    let error = spec.validate().expect_err("bad references fail");
    assert!(has_diagnostic(&error, |diagnostic| matches!(
        diagnostic,
        InteractionSpecDiagnostic::UndeclaredReference {
            expected: InteractionSpecSymbolKind::Responder,
            site: InteractionSpecReferenceSite::ProfileResponder { .. },
            ..
        }
    )));
    assert!(has_diagnostic(&error, |diagnostic| matches!(
        diagnostic,
        InteractionSpecDiagnostic::UndeclaredReference {
            expected: InteractionSpecSymbolKind::ProposalKind,
            site: InteractionSpecReferenceSite::CommandProposalKind { .. },
            ..
        }
    )));
    assert!(has_diagnostic(&error, |diagnostic| matches!(
        diagnostic,
        InteractionSpecDiagnostic::UndeclaredReference {
            expected: InteractionSpecSymbolKind::AcceptanceAction,
            site: InteractionSpecReferenceSite::CommandAcceptanceAction { .. },
            ..
        }
    )));
    assert!(has_diagnostic(&error, |diagnostic| matches!(
        diagnostic,
        InteractionSpecDiagnostic::UndeclaredReference {
            expected: InteractionSpecSymbolKind::Command,
            site: InteractionSpecReferenceSite::AcceptancePolicyCommand { .. },
            ..
        }
    )));
}

#[test]
fn raw_structs_reject_unknown_fields() {
    let mut value = serde_json::to_value(generic_spec("support-agent")).unwrap();
    value["profiles"][0]["transcript"]["extra"] = Value::Bool(true);

    let error = serde_json::from_value::<RawInteractionSpec>(value)
        .expect_err("deny_unknown_fields rejects nested unknown fields");
    assert!(error.to_string().contains("unknown field `extra`"));
}

#[test]
fn validation_catches_alias_conflicts_within_profile() {
    let mut spec = generic_spec("support-agent");
    let profile = &mut spec.profiles[0];
    profile.commands[0].aliases.push(" ".into());
    let mut conflicting = profile.commands[0].clone();
    conflicting.id = "accept-issue-again".into();
    conflicting.aliases = vec!["/accept".into()];
    profile.commands.push(conflicting);

    let error = spec.validate().expect_err("alias problems fail");
    assert!(has_diagnostic(&error, |diagnostic| matches!(
        diagnostic,
        InteractionSpecDiagnostic::EmptyCommandAlias { command, .. } if command == "accept-issue"
    )));
    assert!(has_diagnostic(&error, |diagnostic| matches!(
        diagnostic,
        InteractionSpecDiagnostic::ConflictingCommandAlias {
            alias,
            first_command,
            second_command,
            ..
        } if alias == "/accept" && first_command == "accept-issue" && second_command == "accept-issue-again"
    )));
}

#[test]
fn validation_catches_unsupported_payloads_and_effects() {
    let mut spec = generic_spec("support-agent");
    let profile = &mut spec.profiles[0];
    profile.proposal_kinds[0].payload = "custom_json".into();
    profile.acceptance_actions[0].effects[0].kind = "send_email".into();

    let error = spec.validate().expect_err("unsupported contracts fail");
    assert!(has_diagnostic(&error, |diagnostic| matches!(
        diagnostic,
        InteractionSpecDiagnostic::UnsupportedPayloadContract { payload, .. } if payload == "custom_json"
    )));
    assert!(has_diagnostic(&error, |diagnostic| matches!(
        diagnostic,
        InteractionSpecDiagnostic::UnsupportedEffectKind { kind, .. } if kind == "send_email"
    )));
}

#[test]
fn product_like_values_validate_under_non_product_profile_id() {
    let mut spec = product_manager_fixture();
    let profile = &mut spec.profiles[0];
    profile.id = "release-planner".into();
    profile.participants.agent.display_name = "release-planner".into();

    let validated = spec
        .validate()
        .expect("product-like values are not special-cased by profile id");
    assert_eq!(validated.profiles()[0].id().as_str(), "release-planner");
    assert_eq!(
        validated.profiles()[0].commands()[0].aliases(),
        &["/file".to_string()]
    );
}
