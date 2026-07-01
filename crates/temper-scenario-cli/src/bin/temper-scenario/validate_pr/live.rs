// SPDX-License-Identifier: MPL-2.0

use std::path::Path;

use temper_scenario_core::{
    AcceptanceCriterion, EvidenceEntry, EvidenceKind, ValidatedClaim, ValidationReport,
    ValidationStatus, ValidationVerdict,
};

use crate::basic_delivery;
use crate::run_context::ScenarioRunFacts;

pub(super) fn add_basic_delivery_run(
    report: &mut ValidationReport,
    check_report: &temper_scenario_core::CheckReport,
    facts: &ScenarioRunFacts,
    temper_bin: Option<&Path>,
    artifact_dir: &Path,
) {
    match basic_delivery::run_live_evidence_lines_for_report(
        &check_report.scenario_path,
        temper_bin,
        artifact_dir,
    ) {
        Ok(lines) => {
            let mut details = facts.evidence_details();
            details.extend(lines);
            report.validated_claims.push(
                ValidatedClaim::new(
                    "Live basic-delivery scenario completes successfully.",
                    ValidationStatus::Observed,
                )
                .with_evidence("live scenario run passed"),
            );
            report.acceptance_criteria.push(
                AcceptanceCriterion::new(
                    "A validation-grade live basic-delivery scenario run completes successfully.",
                    ValidationStatus::Satisfied,
                )
                .with_evidence(
                    "live basic-delivery run completed with real Forgejo, host forgejo-runner, standalone temper, and Jig fake LLM agents",
                ),
            );
            report.evidence.push(
                EvidenceEntry::new(
                    EvidenceKind::ScenarioRun,
                    "Live basic-delivery scenario run completed successfully.",
                )
                .with_details(details),
            );
        }
        Err(error) => {
            report.verdict = ValidationVerdict::Failed;
            report.validated_claims.push(
                ValidatedClaim::new(
                    "Live basic-delivery scenario completes successfully.",
                    ValidationStatus::Failed,
                )
                .with_evidence(error.clone()),
            );
            report.acceptance_criteria.push(
                AcceptanceCriterion::new(
                    "A validation-grade live basic-delivery scenario run completes successfully.",
                    ValidationStatus::Failed,
                )
                .with_evidence(error.clone()),
            );
            report.evidence.push(
                EvidenceEntry::new(EvidenceKind::ScenarioRun, "Live scenario run failed.")
                    .with_detail(error),
            );
        }
    }
}

pub(super) fn add_unsupported_run(
    report: &mut ValidationReport,
    facts: &ScenarioRunFacts,
    scenario_name: &str,
) {
    let message = facts.unsupported_live_message(scenario_name);
    report.verdict = ValidationVerdict::Failed;
    report.validated_claims.push(
        ValidatedClaim::new(
            format!("Scenario `{scenario_name}` supports the requested live tier."),
            ValidationStatus::Failed,
        )
        .with_evidence(message.clone()),
    );
    report.acceptance_criteria.push(
        AcceptanceCriterion::new(
            "A requested live scenario run uses a supported live topology.",
            ValidationStatus::Failed,
        )
        .with_evidence(message.clone()),
    );
    report.evidence.push(
        EvidenceEntry::new(
            EvidenceKind::ScenarioRun,
            "Live scenario run is unsupported.",
        )
        .with_detail(message),
    );
}
