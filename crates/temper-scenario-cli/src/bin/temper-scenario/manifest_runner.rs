// SPDX-License-Identifier: MPL-2.0

use std::path::Path;

use temper_scenario_core::load_resolved_manifest_toml;
use temper_testing::live_manifest::ScenarioBundle;

use super::manifest_executor;
use super::run_context::ScenarioRunFacts;
use super::run_evidence;

pub(super) const RUNNER_ID: &str = "manifest";

pub(super) fn run_live_and_print(
    scenario_path: &Path,
    manifest_path: &Path,
    facts: &ScenarioRunFacts,
    temper_bin: Option<&Path>,
    context: &run_evidence::RunEvidenceContext,
) -> Result<run_evidence::RunEvidenceArtifact, String> {
    let bundle = resolve_execution_bundle(scenario_path, manifest_path)?;
    manifest_executor::run_live_and_print(&bundle, facts, temper_bin, context)
}

pub(super) fn run_live_evidence_lines_for_report(
    scenario_path: &Path,
    manifest_path: &Path,
    temper_bin: Option<&Path>,
    artifact_dir: &Path,
) -> Result<Vec<String>, String> {
    let bundle = resolve_execution_bundle(scenario_path, manifest_path)?;
    manifest_executor::run_live_evidence_lines_for_report(&bundle, temper_bin, artifact_dir)
}

fn resolve_execution_bundle(
    scenario_path: &Path,
    manifest_path: &Path,
) -> Result<ScenarioBundle, String> {
    let manifest = load_resolved_manifest_toml(manifest_path).map_err(|error| error.to_string())?;
    ScenarioBundle::from_manifest(
        scenario_path.to_path_buf(),
        manifest_path.to_path_buf(),
        manifest,
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use temper_testing::live_manifest::ConvergenceStrategy;

    use super::*;

    #[test]
    fn renamed_inherited_bundle_keeps_data_selected_execution() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bundle_dir = dir.path().join("renamed-bundle");
        fs::create_dir(&bundle_dir).expect("create bundle dir");
        let manifest_path = bundle_dir.join("scenario.toml");
        fs::write(
            &manifest_path,
            "name = \"an-arbitrary-new-name\"\n\
             intent = \"Renaming does not select live behavior.\"\n\
             [fixtures]\n\
             extends = \"scenarios/basic-delivery\"\n",
        )
        .expect("write inherited manifest");
        let bundle = resolve_execution_bundle(&bundle_dir, &manifest_path)
            .expect("renamed inherited bundle resolves");

        assert_eq!(
            bundle.execution.convergence,
            ConvergenceStrategy::SinglePullRequest
        );
        assert!(
            bundle
                .jig_script_path()
                .ends_with("jig/basic-delivery.json")
        );
        assert_eq!(bundle.repo.slug, "acme/service");
    }
}
