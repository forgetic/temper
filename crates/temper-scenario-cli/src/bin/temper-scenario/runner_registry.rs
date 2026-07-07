// SPDX-License-Identifier: MPL-2.0

use std::path::Path;

use temper_scenario_core::ScenarioManifest;

use super::manifest_runner;
use super::run_context::{ScenarioRunFacts, ScenarioTier};
use super::run_evidence::{RunEvidenceArtifact, RunEvidenceContext};

type LiveRunAndPrint = fn(
    &Path,
    &Path,
    &ScenarioRunFacts,
    Option<&Path>,
    &RunEvidenceContext,
) -> Result<RunEvidenceArtifact, String>;
type LiveEvidenceLines = fn(&Path, &Path, Option<&Path>, &Path) -> Result<Vec<String>, String>;

#[derive(Clone, Copy)]
struct LiveRunner {
    run_and_print: LiveRunAndPrint,
    evidence_lines: LiveEvidenceLines,
    requires_standalone_temper: bool,
}

#[derive(Clone, Copy)]
pub(super) struct RunnerDefinition {
    id: &'static str,
    supported_tiers: &'static [ScenarioTier],
    live: LiveRunner,
}

#[derive(Clone, Copy)]
pub(super) struct SelectedRunner {
    runner: &'static RunnerDefinition,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(super) enum RunnerRegistryError {
    MissingRunnerUses {
        scenario_name: String,
    },
    UnsupportedRunner {
        requested: String,
    },
    UnsupportedTier {
        requested: String,
        runner_id: &'static str,
        tier: ScenarioTier,
        supported_tiers: String,
    },
}

static RUNNERS: &[RunnerDefinition] = &[RunnerDefinition {
    id: manifest_runner::RUNNER_ID,
    supported_tiers: &[ScenarioTier::Live],
    live: LiveRunner {
        run_and_print: manifest_runner::run_live_and_print,
        evidence_lines: manifest_runner::run_live_evidence_lines_for_report,
        requires_standalone_temper: true,
    },
}];

pub(super) fn select_runner(
    manifest: &ScenarioManifest,
    tier: ScenarioTier,
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
    if !runner.supports_tier(tier) {
        return Err(RunnerRegistryError::UnsupportedTier {
            requested: requested.to_string(),
            runner_id: runner.id,
            tier,
            supported_tiers: runner.supported_tiers_display(),
        });
    }
    Ok(SelectedRunner { runner })
}

pub(super) fn supported_runners_display() -> String {
    RUNNERS
        .iter()
        .map(|runner| {
            format!(
                "{} (tiers: {})",
                runner.id,
                runner.supported_tiers_display()
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

impl RunnerDefinition {
    pub(super) fn id(&self) -> &'static str {
        self.id
    }

    pub(super) fn supports_tier(&self, tier: ScenarioTier) -> bool {
        self.supported_tiers.contains(&tier)
    }

    pub(super) fn supported_tiers_display(&self) -> String {
        self.supported_tiers
            .iter()
            .map(|tier| tier.as_str())
            .collect::<Vec<_>>()
            .join(", ")
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
        tier: ScenarioTier,
        temper_bin: Option<&Path>,
        evidence_context: &RunEvidenceContext,
    ) -> Result<RunEvidenceArtifact, String> {
        if tier != ScenarioTier::Live {
            return Err(self.invariant_error(tier));
        }
        (self.runner.live.run_and_print)(
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
        (self.runner.live.evidence_lines)(scenario_path, manifest_path, temper_bin, artifact_dir)
    }

    pub(super) fn selector_key(&self) -> &'static str {
        "runner.uses"
    }

    pub(super) fn requires_standalone_temper(&self, tier: ScenarioTier) -> bool {
        tier == ScenarioTier::Live && self.runner.live.requires_standalone_temper
    }

    fn invariant_error(&self, tier: ScenarioTier) -> String {
        format!(
            "runner registry invariant violated: runner `{}` was selected for unsupported tier `{}`",
            self.runner.id(),
            tier.as_str()
        )
    }
}

impl RunnerRegistryError {
    pub(super) fn message(&self, scenario_path: &Path) -> String {
        match self {
            Self::MissingRunnerUses { scenario_name } => format!(
                "scenario `{scenario_name}` at {} does not declare `[runner] uses = \"manifest\"`; the legacy scenario-name fallback has been removed and Temper will not dispatch by `name`; supported runner ids: {}; use the validation-grade live stack: real Forgejo + real forgejo-runner CI + real Temper + Jig fake LLM",
                scenario_path.display(),
                supported_runners_display()
            ),
            Self::UnsupportedRunner { requested } => format!(
                "unsupported runner `{requested}` selected by runner.uses at {}; supported runner ids: {}; no compatibility aliases are registered; use `runner.uses = \"manifest\"` for the validation-grade live stack: real Forgejo + real forgejo-runner CI + real Temper + Jig fake LLM",
                scenario_path.display(),
                supported_runners_display()
            ),
            Self::UnsupportedTier {
                requested,
                runner_id,
                tier,
                supported_tiers,
            } => format!(
                "unsupported tier `{}` for runner `{runner_id}` selected by runner.uses at {}; requested runner `{requested}`; supported tiers: {supported_tiers}; the manifest runner is validation-grade live only (real Forgejo + real forgejo-runner CI + real Temper + Jig fake LLM) and has no hermetic, MemoryForge, or in-process substitute; refusing to substitute another runner; supported runner ids: {}",
                tier.as_str(),
                scenario_path.display(),
                supported_runners_display()
            ),
        }
    }
}
