// SPDX-License-Identifier: MPL-2.0

//! Internal live-manifest harness adapter.
//!
//! The generic `manifest` runner currently reuses the historical basic-delivery
//! live harness implementation to provision the validation-grade stack and
//! collect evidence. This module is intentionally **not** a public runner id or
//! compatibility alias; `runner_registry` exposes only `runner.uses = "manifest"`.

use std::path::Path;

use super::run_context::ScenarioRunFacts;
use super::run_evidence;

#[path = "basic_delivery/live.rs"]
mod live;
#[path = "basic_delivery/observability.rs"]
mod observability;
#[path = "basic_delivery/plan_artifact.rs"]
mod plan_artifact;

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
