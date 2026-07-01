// SPDX-License-Identifier: MPL-2.0

use std::path::Path;

use temper_scenario_core::{
    AcceptanceCriterion, EvidenceEntry, EvidenceKind, ValidatedClaim, ValidationReport,
    ValidationStatus, ValidationVerdict,
};

use crate::run_context::ScenarioRunFacts;
use crate::runner_registry::SelectedRunner;

pub(super) fn add_supported_run(
    report: &mut ValidationReport,
    check_report: &temper_scenario_core::CheckReport,
    facts: &ScenarioRunFacts,
    selected_runner: &SelectedRunner,
    temper_bin: Option<&Path>,
    artifact_dir: &Path,
) {
    let label = selected_runner.id();
    let Some(manifest_path) = check_report.manifest_path.as_deref() else {
        report.limitations.push(format!(
            "Scenario `{label}` had no resolved manifest path, so no live scenario run occurred."
        ));
        report.acceptance_criteria.push(
            AcceptanceCriterion::new(
                format!("A validation-grade live {label} scenario run completes successfully."),
                ValidationStatus::Unproven,
            )
            .with_evidence("No manifest path was available for the live scenario runner."),
        );
        return;
    };

    match selected_runner.live_evidence_lines(
        &check_report.scenario_path,
        manifest_path,
        temper_bin,
        artifact_dir,
    ) {
        Ok(lines) => {
            let mut details = facts.evidence_details();
            details.push(selected_runner.selection_detail());
            details.extend(lines);
            report.validated_claims.push(
                ValidatedClaim::new(
                    format!("Live {label} scenario completes successfully."),
                    ValidationStatus::Observed,
                )
                .with_evidence("live scenario run passed"),
            );
            report.acceptance_criteria.push(
                AcceptanceCriterion::new(
                    format!(
                        "A validation-grade live {label} scenario run completes successfully."
                    ),
                    ValidationStatus::Satisfied,
                )
                .with_evidence(
                    "live scenario run completed with real Forgejo, host forgejo-runner, standalone temper, and Jig fake LLM agents",
                ),
            );
            report.evidence.push(
                EvidenceEntry::new(
                    EvidenceKind::ScenarioRun,
                    format!("Live {label} scenario run completed successfully."),
                )
                .with_details(details),
            );
        }
        Err(error) => {
            report.verdict = ValidationVerdict::Failed;
            report.validated_claims.push(
                ValidatedClaim::new(
                    format!("Live {label} scenario completes successfully."),
                    ValidationStatus::Failed,
                )
                .with_evidence(error.clone()),
            );
            report.acceptance_criteria.push(
                AcceptanceCriterion::new(
                    format!("A validation-grade live {label} scenario run completes successfully."),
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
