// SPDX-License-Identifier: MPL-2.0

//! Internal live-manifest harness adapter.
//!
//! The generic `manifest` runner resolves a typed bundle before reaching this
//! adapter. The low-level live helpers remain reusable, but runner behavior is
//! selected only by ordered manifest actions and explicit convergence strategy.

use std::path::Path;

use temper_testing::live_manifest::ScenarioBundle;

use super::run_context::ScenarioRunFacts;
use super::run_evidence;

#[path = "manifest_executor/live.rs"]
mod live;
#[path = "manifest_executor/observability.rs"]
mod observability;
#[path = "manifest_executor/plan_artifact.rs"]
mod plan_artifact;

pub(super) fn run_live_and_print(
    scenario: &ScenarioBundle,
    facts: &ScenarioRunFacts,
    temper_bin: Option<&Path>,
    context: &run_evidence::RunEvidenceContext,
) -> Result<run_evidence::RunEvidenceArtifact, String> {
    live::run_and_print(scenario, facts, temper_bin, context)
}

pub(super) fn run_live_evidence_lines_for_report(
    scenario: &ScenarioBundle,
    temper_bin: Option<&Path>,
    artifact_dir: &Path,
) -> Result<Vec<String>, String> {
    live::evidence_lines(scenario, temper_bin, Some(artifact_dir))
}
