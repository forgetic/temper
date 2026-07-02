// SPDX-License-Identifier: MPL-2.0

use std::path::Path;

use super::run_context::ScenarioRunFacts;
use super::run_evidence;

#[path = "basic_delivery/evidence.rs"]
mod evidence;
#[path = "basic_delivery/fixture.rs"]
mod fixture;
#[path = "basic_delivery/live.rs"]
mod live;
#[path = "basic_delivery/model.rs"]
mod model;
#[path = "basic_delivery/render.rs"]
mod render;
#[path = "basic_delivery/runner.rs"]
mod runner;
#[path = "basic_delivery/state.rs"]
mod state;

pub(super) const SCENARIO_NAME: &str = "basic-delivery";

pub(super) fn run_and_print(
    scenario_path: &Path,
    manifest_path: &Path,
    facts: &ScenarioRunFacts,
    context: &run_evidence::RunEvidenceContext,
) -> Result<run_evidence::RunEvidenceArtifact, String> {
    let outcome =
        temper_testing::block_on(runner::run_basic_delivery(scenario_path, manifest_path))?;
    render::print_outcome(&outcome, facts);
    Ok(render::outcome_artifact(&outcome, context))
}

pub(super) fn run_evidence_lines(
    scenario_path: &Path,
    manifest_path: &Path,
) -> Result<Vec<String>, String> {
    let outcome =
        temper_testing::block_on(runner::run_basic_delivery(scenario_path, manifest_path))?;
    Ok(render::outcome_evidence_lines(&outcome))
}

pub(super) fn run_live_and_print(
    scenario_path: &Path,
    manifest_path: &Path,
    facts: &ScenarioRunFacts,
    temper_bin: Option<&Path>,
    context: &run_evidence::RunEvidenceContext,
) -> Result<run_evidence::RunEvidenceArtifact, String> {
    live::run_and_print(scenario_path, manifest_path, facts, temper_bin, context)
}

pub(super) fn run_live_evidence_lines_for_report(
    scenario_path: &Path,
    manifest_path: &Path,
    temper_bin: Option<&Path>,
    artifact_dir: &Path,
) -> Result<Vec<String>, String> {
    live::evidence_lines(scenario_path, manifest_path, temper_bin, Some(artifact_dir))
}

#[cfg(test)]
#[path = "basic_delivery/tests.rs"]
mod tests;
