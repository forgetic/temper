//! Per-profile validation of an interaction spec.
//!
//! Validates one profile's transcript policy, responder reference, proposal
//! kinds, commands, acceptance actions, and their declared effects, emitting
//! diagnostics through the shared helpers in the parent module.

use std::collections::{HashMap, HashSet};

use crate::spec::{
    RawAcceptanceActionDeclaration, RawAcceptanceEffect, RawInteractiveProfile, RawTranscriptPolicy,
};

use super::diagnostics::{
    InteractionSpecDiagnostic, InteractionSpecReferenceSite, InteractionSpecSymbolKind,
};
use super::{check_reference, check_slug, collect_declared};

const TRANSCRIPT_TARGET_ISSUE: &str = "issue";
const TRANSCRIPT_LABEL_POLICY_EXACT: &str = "exact";
const PAYLOAD_CONTRACT_ISSUE_DRAFT: &str = "issue_draft";
const ACCEPTANCE_POLICY_EXPLICIT: &str = "explicit";
const EFFECT_CREATE_ISSUE: &str = "create_issue";
const EFFECT_ADD_TRANSCRIPT_COMMENT: &str = "add_transcript_comment";

pub(super) fn validate_profile(
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
