// SPDX-License-Identifier: MPL-2.0

use std::path::Path;

use temper_scenario_core::{
    AcceptanceCriterion, EvidenceEntry, EvidenceKind, ValidatedClaim, ValidationReport,
    ValidationStatus, ValidationVerdict,
};

use crate::run_context::{ScenarioRunFacts, ScenarioTier};
use crate::runner_registry::{self, RunnerRegistryError};

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
        Ok(selected_runner) => live::add_supported_run(
            report,
            check_report,
            facts,
            &selected_runner,
            temper_bin,
            artifact_dir,
        ),
        Err(error) => add_unsupported_registry_selection(
            report,
            &error,
            &check_report.scenario_path,
            scenario_name,
        ),
    }
}

fn add_unsupported_registry_selection(
    report: &mut ValidationReport,
    error: &RunnerRegistryError,
    scenario_path: &Path,
    scenario_name: &str,
) {
    let message = error.message(scenario_path);
    report.verdict = ValidationVerdict::Failed;

    report.validated_claims.push(
        ValidatedClaim::new(
            format!(
                "Scenario `{scenario_name}` selects the public manifest runner and requested tier."
            ),
            ValidationStatus::Failed,
        )
        .with_evidence(message.clone()),
    );
    report.acceptance_criteria.push(
        AcceptanceCriterion::new(
            "A scenario declares `[runner] uses = \"manifest\"` and runs on the validation-grade live stack.",
            ValidationStatus::Failed,
        )
        .with_evidence(message.clone()),
    );
    report.evidence.push(
        EvidenceEntry::new(
            EvidenceKind::ScenarioRun,
            "Scenario runner selection failed before execution.",
        )
        .with_detail(message.clone()),
    );
    report.limitations.push(format!(
        "No scenario run occurred for `{scenario_name}`; {message}."
    ));
}
