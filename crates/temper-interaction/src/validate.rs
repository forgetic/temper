mod build;
mod diagnostics;
use build::build_validated;
pub use diagnostics::{
    InteractionSpecDiagnostic, InteractionSpecReferenceSite, InteractionSpecSeverity,
    InteractionSpecSymbolKind, InteractionSpecValidationErrors,
};

use crate::spec::{
    RawAcceptanceActionDeclaration, RawAcceptanceEffect, RawInteractionSpec, RawInteractiveProfile,
    RawTranscriptPolicy,
};
use crate::types::is_valid_deterministic_slug;
use crate::validated::ValidatedInteractionSpec;
use crate::DETERMINISTIC_SLUG_RULE;
use std::collections::{HashMap, HashSet};

const TRANSCRIPT_TARGET_ISSUE: &str = "issue";
const TRANSCRIPT_LABEL_POLICY_EXACT: &str = "exact";
const RESPONDER_PROTOCOL_PROCESS_V1: &str = "process-v1";
const PAYLOAD_CONTRACT_ISSUE_DRAFT: &str = "issue_draft";
const ACCEPTANCE_POLICY_EXPLICIT: &str = "explicit";
const EFFECT_CREATE_ISSUE: &str = "create_issue";
const EFFECT_ADD_TRANSCRIPT_COMMENT: &str = "add_transcript_comment";

/// Validates a raw interaction spec into a normalized checked model.
pub fn validate(
    spec: &RawInteractionSpec,
) -> Result<ValidatedInteractionSpec, InteractionSpecValidationErrors> {
    let mut diagnostics = Vec::new();
    check_slug(
        InteractionSpecSymbolKind::Spec,
        None,
        &spec.id,
        &mut diagnostics,
    );

    let responders = collect_declared(
        spec.responders.iter().map(|responder| &responder.id),
        InteractionSpecSymbolKind::Responder,
        None,
        &mut diagnostics,
    );
    for responder in &spec.responders {
        check_slug(
            InteractionSpecSymbolKind::Responder,
            None,
            &responder.id,
            &mut diagnostics,
        );
        if responder.protocol != RESPONDER_PROTOCOL_PROCESS_V1 {
            diagnostics.push(InteractionSpecDiagnostic::UnsupportedResponderProtocol {
                responder: responder.id.clone(),
                protocol: responder.protocol.clone(),
            });
        }
    }

    collect_declared(
        spec.profiles.iter().map(|profile| &profile.id),
        InteractionSpecSymbolKind::Profile,
        None,
        &mut diagnostics,
    );

    for profile in &spec.profiles {
        validate_profile(profile, &responders, &mut diagnostics);
    }

    if diagnostics.is_empty() {
        Ok(build_validated(spec))
    } else {
        Err(InteractionSpecValidationErrors::new(diagnostics))
    }
}

fn validate_profile(
    profile: &RawInteractiveProfile,
    responders: &HashSet<String>,
    diagnostics: &mut Vec<InteractionSpecDiagnostic>,
) {
    check_slug(
        InteractionSpecSymbolKind::Profile,
        None,
        &profile.id,
        diagnostics,
    );
    validate_transcript(&profile.id, &profile.transcript, diagnostics);
    check_reference(
        responders,
        &profile.responder,
        InteractionSpecSymbolKind::Responder,
        InteractionSpecReferenceSite::ProfileResponder {
            profile: profile.id.clone(),
        },
        diagnostics,
    );

    let proposal_kinds = collect_declared(
        profile.proposal_kinds.iter().map(|kind| &kind.id),
        InteractionSpecSymbolKind::ProposalKind,
        Some(&profile.id),
        diagnostics,
    );
    for proposal_kind in &profile.proposal_kinds {
        check_slug(
            InteractionSpecSymbolKind::ProposalKind,
            Some(&profile.id),
            &proposal_kind.id,
            diagnostics,
        );
        if proposal_kind.payload != PAYLOAD_CONTRACT_ISSUE_DRAFT {
            diagnostics.push(InteractionSpecDiagnostic::UnsupportedPayloadContract {
                profile: profile.id.clone(),
                proposal_kind: proposal_kind.id.clone(),
                payload: proposal_kind.payload.clone(),
            });
        }
    }

    let commands = collect_declared(
        profile.commands.iter().map(|command| &command.id),
        InteractionSpecSymbolKind::Command,
        Some(&profile.id),
        diagnostics,
    );
    let acceptance_actions = collect_declared(
        profile.acceptance_actions.iter().map(|action| &action.id),
        InteractionSpecSymbolKind::AcceptanceAction,
        Some(&profile.id),
        diagnostics,
    );

    validate_commands(profile, &proposal_kinds, &acceptance_actions, diagnostics);
    validate_acceptance_actions(profile, &proposal_kinds, &commands, diagnostics);
}

fn validate_transcript(
    profile: &str,
    transcript: &RawTranscriptPolicy,
    diagnostics: &mut Vec<InteractionSpecDiagnostic>,
) {
    if transcript.target != TRANSCRIPT_TARGET_ISSUE {
        diagnostics.push(InteractionSpecDiagnostic::UnsupportedTranscriptTarget {
            profile: profile.to_string(),
            target: transcript.target.clone(),
        });
    }
    if transcript.label_policy != TRANSCRIPT_LABEL_POLICY_EXACT {
        diagnostics.push(
            InteractionSpecDiagnostic::UnsupportedTranscriptLabelPolicy {
                profile: profile.to_string(),
                policy: transcript.label_policy.clone(),
            },
        );
    }
    if transcript.labels.is_empty() {
        diagnostics.push(InteractionSpecDiagnostic::EmptyTranscriptLabels {
            profile: profile.to_string(),
        });
    }
    if transcript
        .labels
        .iter()
        .any(|label| label.trim().is_empty())
    {
        diagnostics.push(InteractionSpecDiagnostic::EmptyTranscriptLabel {
            profile: profile.to_string(),
        });
    }
    if transcript.title_prefix.trim().is_empty() {
        diagnostics.push(InteractionSpecDiagnostic::EmptyTranscriptTitlePrefix {
            profile: profile.to_string(),
        });
    }
    let marker_namespace = transcript.marker_namespace.trim();
    if marker_namespace.is_empty() {
        diagnostics.push(InteractionSpecDiagnostic::EmptyTranscriptMarkerNamespace {
            profile: profile.to_string(),
        });
    } else {
        check_slug(
            InteractionSpecSymbolKind::MarkerNamespace,
            Some(profile),
            marker_namespace,
            diagnostics,
        );
    }
}

fn validate_commands(
    profile: &RawInteractiveProfile,
    proposal_kinds: &HashSet<String>,
    acceptance_actions: &HashSet<String>,
    diagnostics: &mut Vec<InteractionSpecDiagnostic>,
) {
    let mut aliases: HashMap<String, String> = HashMap::new();
    for command in &profile.commands {
        check_slug(
            InteractionSpecSymbolKind::Command,
            Some(&profile.id),
            &command.id,
            diagnostics,
        );
        for alias in &command.aliases {
            let alias = alias.trim();
            if alias.is_empty() {
                diagnostics.push(InteractionSpecDiagnostic::EmptyCommandAlias {
                    profile: profile.id.clone(),
                    command: command.id.clone(),
                });
                continue;
            }
            if let Some(first_command) = aliases.insert(alias.to_string(), command.id.clone()) {
                diagnostics.push(InteractionSpecDiagnostic::ConflictingCommandAlias {
                    profile: profile.id.clone(),
                    alias: alias.to_string(),
                    first_command,
                    second_command: command.id.clone(),
                });
            }
        }

        let action = &command.action.accept_proposal;
        check_reference(
            proposal_kinds,
            &action.kind,
            InteractionSpecSymbolKind::ProposalKind,
            InteractionSpecReferenceSite::CommandProposalKind {
                profile: profile.id.clone(),
                command: command.id.clone(),
            },
            diagnostics,
        );
        check_reference(
            acceptance_actions,
            &action.acceptance_action,
            InteractionSpecSymbolKind::AcceptanceAction,
            InteractionSpecReferenceSite::CommandAcceptanceAction {
                profile: profile.id.clone(),
                command: command.id.clone(),
            },
            diagnostics,
        );
    }
}

fn validate_acceptance_actions(
    profile: &RawInteractiveProfile,
    proposal_kinds: &HashSet<String>,
    commands: &HashSet<String>,
    diagnostics: &mut Vec<InteractionSpecDiagnostic>,
) {
    for action in &profile.acceptance_actions {
        check_slug(
            InteractionSpecSymbolKind::AcceptanceAction,
            Some(&profile.id),
            &action.id,
            diagnostics,
        );
        check_reference(
            proposal_kinds,
            &action.proposal_kind,
            InteractionSpecSymbolKind::ProposalKind,
            InteractionSpecReferenceSite::AcceptanceProposalKind {
                profile: profile.id.clone(),
                acceptance_action: action.id.clone(),
            },
            diagnostics,
        );
        if action.acceptance.policy != ACCEPTANCE_POLICY_EXPLICIT {
            diagnostics.push(InteractionSpecDiagnostic::UnsupportedAcceptancePolicy {
                profile: profile.id.clone(),
                acceptance_action: action.id.clone(),
                policy: action.acceptance.policy.clone(),
            });
        }
        for command in &action.acceptance.commands {
            check_reference(
                commands,
                command,
                InteractionSpecSymbolKind::Command,
                InteractionSpecReferenceSite::AcceptancePolicyCommand {
                    profile: profile.id.clone(),
                    acceptance_action: action.id.clone(),
                },
                diagnostics,
            );
        }
        if action.idempotency_key.trim().is_empty() {
            diagnostics.push(InteractionSpecDiagnostic::EmptyAcceptanceField {
                profile: profile.id.clone(),
                acceptance_action: action.id.clone(),
                field: "idempotency_key",
            });
        }
        if action.effects.is_empty() {
            diagnostics.push(InteractionSpecDiagnostic::EmptyAcceptanceField {
                profile: profile.id.clone(),
                acceptance_action: action.id.clone(),
                field: "effects",
            });
        }
        validate_effects(profile, action, diagnostics);
    }
}

fn validate_effects(
    profile: &RawInteractiveProfile,
    action: &RawAcceptanceActionDeclaration,
    diagnostics: &mut Vec<InteractionSpecDiagnostic>,
) {
    for effect in &action.effects {
        match effect.kind.as_str() {
            EFFECT_CREATE_ISSUE => check_create_issue_effect(profile, action, effect, diagnostics),
            EFFECT_ADD_TRANSCRIPT_COMMENT => {
                check_transcript_comment_effect(profile, action, effect, diagnostics)
            }
            _ => diagnostics.push(InteractionSpecDiagnostic::UnsupportedEffectKind {
                profile: profile.id.clone(),
                acceptance_action: action.id.clone(),
                kind: effect.kind.clone(),
            }),
        }
    }
}

fn check_create_issue_effect(
    profile: &RawInteractiveProfile,
    action: &RawAcceptanceActionDeclaration,
    effect: &RawAcceptanceEffect,
    diagnostics: &mut Vec<InteractionSpecDiagnostic>,
) {
    for (field, empty) in [
        ("title", effect.title.trim().is_empty()),
        ("body_template", effect.body_template.trim().is_empty()),
        (
            "marker_namespace",
            effect.marker_namespace.trim().is_empty(),
        ),
    ] {
        if empty {
            diagnostics.push(InteractionSpecDiagnostic::EmptyAcceptanceField {
                profile: profile.id.clone(),
                acceptance_action: action.id.clone(),
                field,
            });
        }
    }
    if effect.labels.is_empty() || effect.labels.iter().any(|label| label.trim().is_empty()) {
        diagnostics.push(InteractionSpecDiagnostic::EmptyAcceptanceField {
            profile: profile.id.clone(),
            acceptance_action: action.id.clone(),
            field: "labels",
        });
    }
    if effect
        .assignees
        .iter()
        .any(|assignee| assignee.trim().is_empty())
    {
        diagnostics.push(InteractionSpecDiagnostic::EmptyAcceptanceField {
            profile: profile.id.clone(),
            acceptance_action: action.id.clone(),
            field: "assignees",
        });
    }
    check_effect_marker(profile, action, effect, diagnostics);
}

fn check_transcript_comment_effect(
    profile: &RawInteractiveProfile,
    action: &RawAcceptanceActionDeclaration,
    effect: &RawAcceptanceEffect,
    diagnostics: &mut Vec<InteractionSpecDiagnostic>,
) {
    for (field, empty) in [
        ("body_template", effect.body_template.trim().is_empty()),
        (
            "marker_namespace",
            effect.marker_namespace.trim().is_empty(),
        ),
    ] {
        if empty {
            diagnostics.push(InteractionSpecDiagnostic::EmptyAcceptanceField {
                profile: profile.id.clone(),
                acceptance_action: action.id.clone(),
                field,
            });
        }
    }
    check_effect_marker(profile, action, effect, diagnostics);
}

fn check_effect_marker(
    profile: &RawInteractiveProfile,
    _action: &RawAcceptanceActionDeclaration,
    effect: &RawAcceptanceEffect,
    diagnostics: &mut Vec<InteractionSpecDiagnostic>,
) {
    for value in [effect.marker_namespace.trim(), effect.marker_key.trim()] {
        if !value.is_empty() {
            check_slug(
                InteractionSpecSymbolKind::MarkerNamespace,
                Some(&profile.id),
                value,
                diagnostics,
            );
        }
    }
}

fn collect_declared<'a>(
    ids: impl Iterator<Item = &'a String>,
    kind: InteractionSpecSymbolKind,
    profile: Option<&str>,
    diagnostics: &mut Vec<InteractionSpecDiagnostic>,
) -> HashSet<String> {
    let mut seen = HashSet::new();
    for id in ids {
        if !seen.insert(id.clone()) {
            diagnostics.push(InteractionSpecDiagnostic::DuplicateId {
                kind,
                profile: profile.map(str::to_string),
                id: id.clone(),
            });
        }
    }
    seen
}

fn check_reference(
    declared: &HashSet<String>,
    id: &str,
    expected: InteractionSpecSymbolKind,
    site: InteractionSpecReferenceSite,
    diagnostics: &mut Vec<InteractionSpecDiagnostic>,
) {
    if !declared.contains(id) {
        diagnostics.push(InteractionSpecDiagnostic::UndeclaredReference {
            expected,
            id: id.to_string(),
            site,
        });
    }
}

fn check_slug(
    kind: InteractionSpecSymbolKind,
    profile: Option<&str>,
    value: &str,
    diagnostics: &mut Vec<InteractionSpecDiagnostic>,
) {
    if !is_valid_deterministic_slug(value) {
        diagnostics.push(InteractionSpecDiagnostic::InvalidSlug {
            kind,
            profile: profile.map(str::to_string),
            value: value.to_string(),
            reason: DETERMINISTIC_SLUG_RULE,
        });
    }
}
