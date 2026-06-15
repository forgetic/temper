mod build;
mod diagnostics;
mod profile;
use build::build_validated;
pub use diagnostics::{
    InteractionSpecDiagnostic, InteractionSpecReferenceSite, InteractionSpecSeverity,
    InteractionSpecSymbolKind, InteractionSpecValidationErrors,
};
use profile::validate_profile;

use crate::DETERMINISTIC_SLUG_RULE;
use crate::spec::RawInteractionSpec;
use crate::types::is_valid_deterministic_slug;
use crate::validated::ValidatedInteractionSpec;
use std::collections::HashSet;

const RESPONDER_PROTOCOL_PROCESS_V1: &str = "process-v1";

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
