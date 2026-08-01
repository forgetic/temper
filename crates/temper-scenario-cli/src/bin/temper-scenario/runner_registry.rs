// SPDX-License-Identifier: MPL-2.0

use std::path::Path;

use temper_scenario_core::ScenarioManifest;

use super::manifest_runner;
use super::run_context::{LIVE_TOPOLOGY_DESCRIPTION, ScenarioRunFacts};
use super::run_evidence::{RunEvidenceArtifact, RunEvidenceContext};

type RunAndPrint = fn(
    &Path,
    &Path,
    &ScenarioRunFacts,
    Option<&Path>,
    &RunEvidenceContext,
) -> Result<RunEvidenceArtifact, String>;
type EvidenceLines = fn(&Path, &Path, Option<&Path>, &Path) -> Result<Vec<String>, String>;

#[derive(Clone, Copy)]
pub(super) struct RunnerDefinition {
    id: &'static str,
    run_and_print: RunAndPrint,
    evidence_lines: EvidenceLines,
}

#[derive(Clone, Copy)]
pub(super) struct SelectedRunner {
    runner: &'static RunnerDefinition,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(super) enum RunnerRegistryError {
    MissingRunnerUses { scenario_name: String },
    UnsupportedRunner { requested: String },
}

static RUNNERS: &[RunnerDefinition] = &[RunnerDefinition {
    id: manifest_runner::RUNNER_ID,
    run_and_print: manifest_runner::run_live_and_print,
    evidence_lines: manifest_runner::run_live_evidence_lines_for_report,
}];

pub(super) fn select_runner(
    manifest: &ScenarioManifest,
) -> Result<SelectedRunner, RunnerRegistryError> {
    let Some(requested) = manifest.runner.uses.as_deref() else {
        return Err(RunnerRegistryError::MissingRunnerUses {
            scenario_name: manifest.name.clone(),
        });
    };
    let Some(runner) = RUNNERS.iter().find(|runner| runner.id == requested) else {
        return Err(RunnerRegistryError::UnsupportedRunner {
            requested: requested.to_string(),
        });
    };
    Ok(SelectedRunner { runner })
}

pub(super) fn supported_runners_display() -> String {
    RUNNERS
        .iter()
        .map(|runner| runner.id)
        .collect::<Vec<_>>()
        .join(", ")
}

impl RunnerDefinition {
    pub(super) fn id(&self) -> &'static str {
        self.id
    }
}

impl SelectedRunner {
    pub(super) fn id(&self) -> &'static str {
        self.runner.id()
    }

    pub(super) fn selection_detail(&self) -> String {
        format!("runner: `{}` selected by runner.uses", self.runner.id())
    }

    pub(super) fn run_and_print(
        &self,
        scenario_path: &Path,
        manifest_path: &Path,
        facts: &ScenarioRunFacts,
        temper_bin: Option<&Path>,
        evidence_context: &RunEvidenceContext,
    ) -> Result<RunEvidenceArtifact, String> {
        (self.runner.run_and_print)(
            scenario_path,
            manifest_path,
            facts,
            temper_bin,
            evidence_context,
        )
    }

    pub(super) fn live_evidence_lines(
        &self,
        scenario_path: &Path,
        manifest_path: &Path,
        temper_bin: Option<&Path>,
        artifact_dir: &Path,
    ) -> Result<Vec<String>, String> {
        (self.runner.evidence_lines)(scenario_path, manifest_path, temper_bin, artifact_dir)
    }

    pub(super) fn selector_key(&self) -> &'static str {
        "runner.uses"
    }
}

impl RunnerRegistryError {
    pub(super) fn message(&self, scenario_path: &Path) -> String {
        match self {
            Self::MissingRunnerUses { scenario_name } => format!(
                "scenario `{scenario_name}` at {} does not declare `[runner] uses = \"manifest\"`; the legacy scenario-name fallback has been removed and Temper will not dispatch by `name`; supported runner ids: {}; use the validation-grade topology: {LIVE_TOPOLOGY_DESCRIPTION}",
                scenario_path.display(),
                supported_runners_display()
            ),
            Self::UnsupportedRunner { requested } => format!(
                "unsupported runner `{requested}` selected by runner.uses at {}; supported runner ids: {}; no compatibility aliases are registered; use `runner.uses = \"manifest\"` for the validation-grade topology: {LIVE_TOPOLOGY_DESCRIPTION}",
                scenario_path.display(),
                supported_runners_display()
            ),
        }
    }
}
