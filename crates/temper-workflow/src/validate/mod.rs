//! Static validation from a [`RawWorkflowSpec`] to a [`ValidatedWorkflow`].
//!
//! Validation is diagnostic-collecting: it walks the whole spec and records
//! every problem before deciding whether the workflow is valid. Phase 2 covers
//! duplicate ids and undeclared references; later phases will add semantic
//! checks (contradictory effects, unsatisfiable gates, tool-authority limits).
//!
//! The walk is split by responsibility: this root collects declared symbols and
//! drives the passes, [`references`] resolves cross-symbol references, and
//! [`contracts`] enforces the semantic consistency contracts (queue automation
//! and transition outcome routing).

mod contracts;
mod references;

use crate::ValidationErrors;
use crate::diagnostics::{Diagnostic, ReferenceSite, SymbolKind};
use crate::spec::RawWorkflowSpec;
use crate::validate_build::build_validated;
use crate::validated::ValidatedWorkflow;
use std::collections::HashSet;

/// Validates a raw workflow spec, collecting all diagnostics.
///
/// On success, returns a [`ValidatedWorkflow`]. On failure, returns every
/// detected problem so the author can fix them in one pass.
pub fn validate(spec: &RawWorkflowSpec) -> Result<ValidatedWorkflow, ValidationErrors> {
    let mut diagnostics = Vec::new();

    // Declared-symbol sets double as duplicate detectors and reference targets.
    let roles = collect_declared(
        spec.roles.iter().map(|r| &r.id),
        SymbolKind::Role,
        &mut diagnostics,
    );
    let labels = collect_declared(
        spec.labels.iter().map(|l| &l.id),
        SymbolKind::Label,
        &mut diagnostics,
    );
    let artifacts = collect_declared(
        spec.artifact_kinds.iter().map(|a| &a.id),
        SymbolKind::ArtifactKind,
        &mut diagnostics,
    );
    // State dimension ids are duplicate-checked and referenced by external
    // gate conditions.
    let state_dimensions = collect_declared(
        spec.state_dimensions.iter().map(|d| &d.id),
        SymbolKind::StateDimension,
        &mut diagnostics,
    );
    let queues = collect_declared(
        spec.queues.iter().map(|q| &q.id),
        SymbolKind::Queue,
        &mut diagnostics,
    );
    let transitions = collect_declared(
        spec.transitions.iter().map(|t| &t.id),
        SymbolKind::Transition,
        &mut diagnostics,
    );
    let gates = collect_declared(
        spec.gates.iter().map(|g| &g.id),
        SymbolKind::Gate,
        &mut diagnostics,
    );
    let _validation_bindings = collect_declared(
        spec.validation_bindings.iter().map(|binding| &binding.id),
        SymbolKind::ValidationBinding,
        &mut diagnostics,
    );

    references::check_references(
        spec,
        &Declared {
            roles: &roles,
            labels: &labels,
            artifacts: &artifacts,
            state_dimensions: &state_dimensions,
            queues: &queues,
            transitions: &transitions,
            gates: &gates,
        },
        &mut diagnostics,
    );

    references::check_state_dimensions(spec, &labels, &artifacts, &mut diagnostics);
    contracts::check_role_external_tools(spec, &mut diagnostics);
    contracts::check_default_artifact_kinds(spec, &mut diagnostics);
    contracts::check_queue_automation_contract(spec, &roles, &mut diagnostics);
    contracts::check_queue_action_contract(spec, &roles, &mut diagnostics);
    contracts::check_validation_binding_contract(spec, &roles, &artifacts, &mut diagnostics);
    contracts::check_create_pull_request_artifact_kind_targets(spec, &mut diagnostics);
    contracts::check_create_issues_cardinality(spec, &mut diagnostics);
    contracts::check_transition_outcome_contract(spec, &mut diagnostics);

    if diagnostics.is_empty() {
        Ok(build_validated(spec))
    } else {
        Err(ValidationErrors::new(diagnostics))
    }
}

/// Declared symbol sets used to resolve references.
struct Declared<'a> {
    roles: &'a HashSet<String>,
    labels: &'a HashSet<String>,
    artifacts: &'a HashSet<String>,
    state_dimensions: &'a HashSet<String>,
    queues: &'a HashSet<String>,
    transitions: &'a HashSet<String>,
    gates: &'a HashSet<String>,
}

/// Collects declared ids, recording a [`Diagnostic::DuplicateId`] per repeat.
fn collect_declared<'a>(
    ids: impl Iterator<Item = &'a String>,
    kind: SymbolKind,
    diagnostics: &mut Vec<Diagnostic>,
) -> HashSet<String> {
    let mut seen = HashSet::new();
    for id in ids {
        if !seen.insert(id.clone()) {
            diagnostics.push(Diagnostic::DuplicateId {
                kind,
                id: id.clone(),
            });
        }
    }
    seen
}

/// Records an undeclared-reference diagnostic when `id` is not declared.
fn check_reference(
    declared: &HashSet<String>,
    id: &str,
    expected: SymbolKind,
    site: ReferenceSite,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !declared.contains(id) {
        diagnostics.push(Diagnostic::UndeclaredReference {
            expected,
            id: id.to_string(),
            site,
        });
    }
}
