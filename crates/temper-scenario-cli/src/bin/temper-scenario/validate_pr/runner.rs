// SPDX-License-Identifier: MPL-2.0

use std::path::Path;

use temper_scenario_core::{
    AcceptanceCriterion, EvidenceEntry, EvidenceKind, ValidatedClaim, ValidationReport,
    ValidationStatus, ValidationVerdict,
};

use crate::run_context::{ScenarioRunFacts, ScenarioTier};
use crate::runner_registry::{self, RunnerRegistryError, SelectedRunner};

use super::live;

pub(super) fn add_scenario_run(
    report: &mut ValidationReport,
    check_report: &temper_scenario_core::CheckReport,
    facts: &ScenarioRunFacts,
    scenario_name: &str,
    tier: ScenarioTier,
    temper_bin: Option<&Path>,
    artifact_dir: &Path,
) {
    let manifest = match check_report.manifest.as_ref() {
        Some(manifest) => manifest,
        None => {
            report.limitations.push(format!(
                "No scenario run occurred for `{scenario_name}`; the checked scenario had no parsed manifest."
            ));
            return;
        }
    };

    match runner_registry::select_runner(manifest, tier) {
        Ok(selected_runner) => match tier {
            ScenarioTier::Hermetic => {
                add_supported_run(report, check_report, facts, scenario_name, &selected_runner)
            }
            ScenarioTier::Live => live::add_supported_run(
                report,
                check_report,
                facts,
                &selected_runner,
                temper_bin,
                artifact_dir,
            ),
        },
        Err(error) => add_unsupported_registry_selection(
            report,
            &error,
            &check_report.scenario_path,
            scenario_name,
            tier,
        ),
    }
}

fn add_unsupported_registry_selection(
    report: &mut ValidationReport,
    error: &RunnerRegistryError,
    scenario_path: &Path,
    scenario_name: &str,
    tier: ScenarioTier,
) {
    let message = error.message(scenario_path);
    let failed = tier == ScenarioTier::Live || !error.is_unsupported_runner();
    let status = if failed {
        report.verdict = ValidationVerdict::Failed;
        ValidationStatus::Failed
    } else {
        ValidationStatus::NotApplicable
    };

    report.validated_claims.push(
        ValidatedClaim::new(
            format!("Scenario `{scenario_name}` has a supported runner for the requested tier."),
            status,
        )
        .with_evidence(message.clone()),
    );
    report.acceptance_criteria.push(
        AcceptanceCriterion::new(
            "A supported scenario runner is available for the requested tier.",
            status,
        )
        .with_evidence(message.clone()),
    );
    report.evidence.push(
        EvidenceEntry::new(
            EvidenceKind::ScenarioRun,
            "Scenario runner selection was unsupported.",
        )
        .with_detail(message.clone()),
    );
    report.limitations.push(format!(
        "No scenario run occurred for `{scenario_name}`; {message}."
    ));
}

fn add_supported_run(
    report: &mut ValidationReport,
    check_report: &temper_scenario_core::CheckReport,
    facts: &ScenarioRunFacts,
    scenario_name: &str,
    selected_runner: &SelectedRunner,
) {
    let Some(manifest_path) = check_report.manifest_path.as_deref() else {
        report.limitations.push(format!(
            "Scenario `{scenario_name}` had no resolved manifest path, so no scenario run occurred."
        ));
        report.acceptance_criteria.push(
            AcceptanceCriterion::new(
                "The supported deterministic scenario completes successfully.",
                ValidationStatus::Unproven,
            )
            .with_evidence("No manifest path was available for the scenario runner."),
        );
        return;
    };

    match selected_runner.hermetic_evidence_lines(&check_report.scenario_path, manifest_path) {
        Ok(lines) => {
            let label = selected_runner.id();
            let mut details = facts.evidence_details();
            details.push(selected_runner.selection_detail());
            details.extend(lines);
            report.validated_claims.push(
                ValidatedClaim::new(
                    format!("Supported deterministic {label} scenario completes successfully."),
                    ValidationStatus::Observed,
                )
                .with_evidence("scenario run passed"),
            );
            report.acceptance_criteria.push(
                AcceptanceCriterion::new(
                    "A supported deterministic scenario run completes successfully.",
                    ValidationStatus::Satisfied,
                )
                .with_evidence(format!("{label} run completed in process")),
            );
            report.evidence.push(
                EvidenceEntry::new(
                    EvidenceKind::ScenarioRun,
                    format!("Deterministic {label} scenario run completed successfully."),
                )
                .with_details(details),
            );
        }
        Err(error) => {
            let label = selected_runner.id();
            report.verdict = ValidationVerdict::Failed;
            report.validated_claims.push(
                ValidatedClaim::new(
                    format!("Supported deterministic {label} scenario completes successfully."),
                    ValidationStatus::Failed,
                )
                .with_evidence(error.clone()),
            );
            report.acceptance_criteria.push(
                AcceptanceCriterion::new(
                    "A supported deterministic scenario run completes successfully.",
                    ValidationStatus::Failed,
                )
                .with_evidence(error.clone()),
            );
            report.evidence.push(
                EvidenceEntry::new(EvidenceKind::ScenarioRun, "Scenario run failed.")
                    .with_detail(error),
            );
        }
    }
}
