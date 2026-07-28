// SPDX-License-Identifier: MPL-2.0

use temper_scenario_core::{
    ArtifactReference, EvidenceKind, ValidationAssertion, ValidationStatus, ValidationVerdict,
    ValidatorResult, ValidatorResultTarget,
};

pub(super) fn apply_issue_target(result: &mut ValidatorResult, kind: &str, issue: u64, repo: &str) {
    result.target = ValidatorResultTarget::new(kind, repo, ArtifactReference::issue(issue));
    result.target.trigger_reason = Some("workflow-native exact-head validation".to_string());
    result.related_prs.clear();

    let evidence_ids = result
        .evidence
        .iter()
        .map(|entry| entry.id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    result.acceptance_criteria.retain(|criterion| {
        !criterion.evidence_refs.is_empty()
            && criterion
                .evidence_refs
                .iter()
                .all(|reference| evidence_ids.contains(reference))
    });
    result.validated_claims.clear();
    if let Some(evidence) = result
        .evidence
        .iter()
        .find(|entry| entry.kind == EvidenceKind::ScenarioRun)
    {
        result.validated_claims.push(
            ValidationAssertion::new(
                "The mapped scenario ran with structured evidence at the exact checkout head.",
                ValidationStatus::Observed,
            )
            .with_evidence_ref(evidence.id.clone()),
        );
    }
    result
        .limitations
        .retain(|limitation| !limitation.starts_with("Temporary validate-pr"));

    let assertions_passed = result
        .validated_claims
        .iter()
        .chain(result.acceptance_criteria.iter())
        .filter(|assertion| assertion.required)
        .all(|assertion| {
            matches!(
                assertion.status,
                ValidationStatus::Satisfied | ValidationStatus::Observed
            )
        });
    let exact_identity_complete = [
        result.feature.as_deref(),
        result.plan.as_deref(),
        result.mapping_id.as_deref(),
        result.scenario_name.as_deref(),
        result.scenario_path.as_deref(),
        result.source_branch.as_deref(),
        result.exact_head_sha.as_deref(),
        result.resolved_content_digest.as_deref(),
    ]
    .into_iter()
    .all(|value| value.is_some_and(|value| !value.trim().is_empty()));
    if result.verdict != ValidationVerdict::Failed
        && exact_identity_complete
        && !result.validated_claims.is_empty()
        && !result.acceptance_criteria.is_empty()
        && assertions_passed
    {
        result.verdict = ValidationVerdict::Passed;
    }
}
