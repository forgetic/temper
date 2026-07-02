// SPDX-License-Identifier: MPL-2.0

use std::path::Path;

use temper_scenario_core::{
    AcceptanceCriterion, EvidenceEntry, EvidenceKind, ValidatedClaim, ValidationReport,
    ValidationStatus, ValidationVerdict, check_scenario,
};

use crate::run_context::{ScenarioRunFacts, ScenarioTier};

use super::Error;

pub(super) fn add_run_evidence_validation(
    report: &mut ValidationReport,
    path: &Path,
    scenario_path: Option<&Path>,
    tier: ScenarioTier,
    tier_explicit: bool,
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

    add_manifest_assertion_validation(report, &loaded.artifact);
    compare_run_evidence(report, &loaded.artifact, scenario_path, tier, tier_explicit)?;
    Ok(())
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
    let status = if assertions.has_failures() {
        report.verdict = ValidationVerdict::Failed;
        ValidationStatus::Failed
    } else {
        ValidationStatus::Observed
    };
    report.validated_claims.push(
        ValidatedClaim::new(
            "Manifest assertions declared by the scenario have no failing results.",
            status,
        )
        .with_evidence(summary.clone()),
    );
    report.acceptance_criteria.push(
        AcceptanceCriterion::new(
            "Declarative manifest expectations are evaluated from structured run evidence.",
            if assertions.has_failures() {
                ValidationStatus::Failed
            } else {
                ValidationStatus::Satisfied
            },
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
    tier: ScenarioTier,
    tier_explicit: bool,
) -> Result<(), Error> {
    let mut matches = Vec::new();
    let mut mismatches = Vec::new();

    if tier_explicit || scenario_path.is_some() {
        if artifact.scenario.tier == tier.as_str() {
            matches.push(format!("tier matches requested `{}`", tier.as_str()));
        } else {
            mismatches.push(format!(
                "tier mismatch: requested `{}`, evidence has `{}`",
                tier.as_str(),
                artifact.scenario.tier
            ));
        }
    } else {
        matches.push(format!(
            "tier accepted from evidence `{}` (no --tier supplied)",
            artifact.scenario.tier
        ));
    }

    if let Some(path) = scenario_path {
        let check_report = check_scenario(path);
        if !check_report.is_valid() {
            return Err(Error::InvalidScenario(Box::new(check_report)));
        }
        let scenario_name = check_report
            .manifest
            .as_ref()
            .map(|manifest| manifest.name.as_str())
            .unwrap_or("unknown");
        let facts = ScenarioRunFacts::from_check_report(&check_report, tier);
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
            match crate::runner_registry::select_runner(manifest, tier) {
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
    } else {
        report.limitations.push(
            "No --scenario path was supplied with --run-evidence; the report did not re-check the scenario manifest."
                .to_string(),
        );
    }

    if mismatches.is_empty() {
        report.validated_claims.push(
            ValidatedClaim::new(
                "Run evidence matches the requested validation scenario context.",
                ValidationStatus::Observed,
            )
            .with_evidence(matches.join("; ")),
        );
        report.acceptance_criteria.push(
            AcceptanceCriterion::new(
                "Run evidence scenario, tier, and runner agree with supplied validation inputs when those inputs are present.",
                ValidationStatus::Satisfied,
            )
            .with_evidence(matches.join("; ")),
        );
        report.evidence.push(
            EvidenceEntry::new(
                EvidenceKind::Observation,
                "Run evidence context matched supplied validation inputs.",
            )
            .with_details(matches),
        );
    } else {
        report.verdict = ValidationVerdict::Failed;
        report.validated_claims.push(
            ValidatedClaim::new(
                "Run evidence matches the requested validation scenario context.",
                ValidationStatus::Failed,
            )
            .with_evidence(mismatches.join("; ")),
        );
        report.acceptance_criteria.push(
            AcceptanceCriterion::new(
                "Run evidence scenario, tier, and runner agree with supplied validation inputs when those inputs are present.",
                ValidationStatus::Failed,
            )
            .with_evidence(mismatches.join("; ")),
        );
        report.evidence.push(
            EvidenceEntry::new(
                EvidenceKind::Observation,
                "Run evidence context did not match supplied validation inputs.",
            )
            .with_details(mismatches),
        );
    }
    Ok(())
}
