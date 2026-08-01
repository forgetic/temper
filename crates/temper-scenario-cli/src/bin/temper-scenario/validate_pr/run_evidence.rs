// SPDX-License-Identifier: MPL-2.0

use std::path::Path;

use temper_scenario_core::{
    AcceptanceCriterion, EvidenceEntry, EvidenceKind, ValidatedClaim, ValidationReport,
    ValidationStatus, ValidationVerdict, check_scenario,
};

use crate::run_context::ScenarioRunFacts;

use super::Error;

pub(super) fn add_run_evidence_validation(
    report: &mut ValidationReport,
    path: &Path,
    scenario_path: Option<&Path>,
) -> Result<(), Error> {
    let loaded = crate::run_evidence::load_run_evidence(path).map_err(Error::RunEvidence)?;
    let diagnostics = loaded.artifact.validate();
    if diagnostics.is_empty() {
        report.validated_claims.push(
            ValidatedClaim::new(
                format!(
                    "Run evidence artifact for scenario `{}` validates against the supported schema.",
                    loaded.artifact.scenario.name
                ),
                ValidationStatus::Observed,
            )
            .with_evidence(format!("loaded `{}`", loaded.path.display())),
        );
        report.acceptance_criteria.push(
            AcceptanceCriterion::new(
                "A previous scenario run-evidence artifact can be loaded without rerunning the scenario.",
                ValidationStatus::Satisfied,
            )
            .with_evidence("run evidence schema and required data validated"),
        );
        report.evidence.push(
            EvidenceEntry::new(
                EvidenceKind::ScenarioRun,
                "Run evidence artifact ingested; scenario run was not rerun.",
            )
            .with_details(loaded.artifact.report_details(&loaded.path)),
        );
    } else {
        report.verdict = ValidationVerdict::Failed;
        report.validated_claims.push(
            ValidatedClaim::new(
                "Run evidence artifact validates against the supported schema.",
                ValidationStatus::Failed,
            )
            .with_evidence(diagnostics.join("; ")),
        );
        report.acceptance_criteria.push(
            AcceptanceCriterion::new(
                "A previous scenario run-evidence artifact can be loaded without rerunning the scenario.",
                ValidationStatus::Failed,
            )
            .with_evidence(diagnostics.join("; ")),
        );
        report.evidence.push(
            EvidenceEntry::new(
                EvidenceKind::ScenarioRun,
                "Run evidence artifact is invalid.",
            )
            .with_details(diagnostics.clone())
            .with_detail(format!("artifact: `{}`", loaded.path.display())),
        );
        report
            .limitations
            .push("The supplied run evidence artifact was malformed or incomplete.".to_string());
        return Ok(());
    }

    add_execution_verdict_validation(report, &loaded.artifact);
    add_manifest_assertion_validation(report, &loaded.artifact);
    compare_run_evidence(report, &loaded.artifact, scenario_path)?;
    Ok(())
}

fn add_execution_verdict_validation(
    report: &mut ValidationReport,
    artifact: &crate::run_evidence::RunEvidenceArtifact,
) {
    match artifact.verdict {
        crate::run_evidence::RunEvidenceVerdict::Passed => {}
        crate::run_evidence::RunEvidenceVerdict::Failed => {
            report.verdict = ValidationVerdict::Failed;
            report.limitations.extend(artifact.limitations.clone());
            report.validated_claims.push(
                ValidatedClaim::new(
                    "The scenario executor completed successfully.",
                    ValidationStatus::Failed,
                )
                .with_evidence("run evidence verdict: failed"),
            );
        }
        crate::run_evidence::RunEvidenceVerdict::Inconclusive => {
            if report.verdict != ValidationVerdict::Failed {
                report.verdict = ValidationVerdict::Inconclusive;
            }
            report.limitations.extend(artifact.limitations.clone());
        }
    }
}

fn add_manifest_assertion_validation(
    report: &mut ValidationReport,
    artifact: &crate::run_evidence::RunEvidenceArtifact,
) {
    let Some(assertions) = artifact.assertions.as_ref() else {
        report.limitations.push(
            "Run evidence artifact did not contain manifest assertion results; rerun with a newer temper-scenario run to populate them."
                .to_string(),
        );
        return;
    };

    let summary = assertions.summary();
    let (claim_status, criterion_status) = match assertions.verdict() {
        crate::run_evidence::RunEvidenceVerdict::Failed => {
            report.verdict = ValidationVerdict::Failed;
            (ValidationStatus::Failed, ValidationStatus::Failed)
        }
        crate::run_evidence::RunEvidenceVerdict::Inconclusive => {
            if report.verdict != ValidationVerdict::Failed {
                report.verdict = ValidationVerdict::Inconclusive;
            }
            (ValidationStatus::Unproven, ValidationStatus::Unproven)
        }
        crate::run_evidence::RunEvidenceVerdict::Passed => {
            (ValidationStatus::Observed, ValidationStatus::Satisfied)
        }
    };
    report.validated_claims.push(
        ValidatedClaim::new(
            "All required manifest assertions declared by the scenario passed.",
            claim_status,
        )
        .with_evidence(summary.clone()),
    );
    report.acceptance_criteria.push(
        AcceptanceCriterion::new(
            "Declarative manifest expectations are evaluated from structured run evidence.",
            criterion_status,
        )
        .with_evidence(summary),
    );
    report.evidence.push(
        EvidenceEntry::new(
            EvidenceKind::ScenarioRun,
            "Manifest assertion results were ingested from run evidence.",
        )
        .with_details(assertions.report_details()),
    );
}

fn compare_run_evidence(
    report: &mut ValidationReport,
    artifact: &crate::run_evidence::RunEvidenceArtifact,
    scenario_path: Option<&Path>,
) -> Result<(), Error> {
    let Some(path) = scenario_path else {
        report.limitations.push(
            "No --scenario path was supplied with --run-evidence; the report did not re-check the scenario manifest."
                .to_string(),
        );
        return Ok(());
    };

    let mut matches = Vec::new();
    let mut mismatches = Vec::new();
    let check_report = check_scenario(path);
    if !check_report.is_valid() {
        return Err(Error::InvalidScenario(Box::new(check_report)));
    }
    let scenario_name = check_report
        .manifest
        .as_ref()
        .map(|manifest| manifest.name.as_str())
        .unwrap_or("unknown");
    let facts = ScenarioRunFacts::from_check_report(&check_report);
    let mut check_evidence = EvidenceEntry::new(
        EvidenceKind::ScenarioCheck,
        "Scenario check passed for comparison with run evidence.",
    )
    .with_detail(format!("scenario: `{scenario_name}`"));
    if let Some(manifest_path) = check_report.manifest_path.as_deref() {
        check_evidence = check_evidence.with_detail(format!(
            "manifest: `{}`",
            crate::display_path(manifest_path)
        ));
    }
    check_evidence = check_evidence.with_details(facts.evidence_details());
    report.evidence.push(check_evidence);

    if scenario_name == artifact.scenario.name {
        matches.push(format!("scenario matches `{scenario_name}`"));
    } else {
        mismatches.push(format!(
            "scenario mismatch: supplied scenario `{scenario_name}`, evidence has `{}`",
            artifact.scenario.name
        ));
    }
    if facts.source.evidence_value() == artifact.scenario.source {
        matches.push(format!(
            "source classification matches `{}`",
            facts.source.as_str()
        ));
    } else {
        mismatches.push(format!(
            "source mismatch: supplied scenario is `{}`, evidence has `{}`",
            facts.source.as_str(),
            artifact.scenario.source_description
        ));
    }
    if let Some(manifest) = check_report.manifest.as_ref() {
        match crate::runner_registry::select_runner(manifest) {
            Ok(selected_runner) if selected_runner.id() == artifact.scenario.runner_id => {
                matches.push(format!("runner matches `{}`", selected_runner.id()));
            }
            Ok(selected_runner) => mismatches.push(format!(
                "runner mismatch: supplied scenario selects `{}`, evidence has `{}`",
                selected_runner.id(),
                artifact.scenario.runner_id
            )),
            Err(error) => mismatches.push(format!(
                "runner mismatch: supplied scenario runner selection failed: {}",
                error.message(&check_report.scenario_path)
            )),
        }
    }

    let criterion = "Run evidence scenario name, source classification, and runner identity agree with the supplied scenario.";
    if mismatches.is_empty() {
        report.validated_claims.push(
            ValidatedClaim::new(
                "Run evidence matches the supplied validation scenario context.",
                ValidationStatus::Observed,
            )
            .with_evidence(matches.join("; ")),
        );
        report.acceptance_criteria.push(
            AcceptanceCriterion::new(criterion, ValidationStatus::Satisfied)
                .with_evidence(matches.join("; ")),
        );
        report.evidence.push(
            EvidenceEntry::new(
                EvidenceKind::Observation,
                "Run evidence context matched the supplied validation scenario.",
            )
            .with_details(matches),
        );
    } else {
        report.verdict = ValidationVerdict::Failed;
        report.validated_claims.push(
            ValidatedClaim::new(
                "Run evidence matches the supplied validation scenario context.",
                ValidationStatus::Failed,
            )
            .with_evidence(mismatches.join("; ")),
        );
        report.acceptance_criteria.push(
            AcceptanceCriterion::new(criterion, ValidationStatus::Failed)
                .with_evidence(mismatches.join("; ")),
        );
        report.evidence.push(
            EvidenceEntry::new(
                EvidenceKind::Observation,
                "Run evidence context did not match the supplied validation scenario.",
            )
            .with_details(mismatches),
        );
    }
    Ok(())
}
