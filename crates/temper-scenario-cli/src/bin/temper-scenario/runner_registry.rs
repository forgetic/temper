// SPDX-License-Identifier: MPL-2.0

use std::path::Path;

use temper_scenario_core::ScenarioManifest;

use super::run_context::{ScenarioRunFacts, ScenarioTier};
use super::run_evidence::{RunEvidenceArtifact, RunEvidenceContext};
use super::{basic_delivery, implementation_pr_handoff};

type HermeticRunAndPrint =
    fn(&Path, &Path, &ScenarioRunFacts, &RunEvidenceContext) -> Result<RunEvidenceArtifact, String>;
type HermeticEvidenceLines = fn(&Path, &Path) -> Result<Vec<String>, String>;
type LiveRunAndPrint = fn(
    &Path,
    &Path,
    &ScenarioRunFacts,
    Option<&Path>,
    &RunEvidenceContext,
) -> Result<RunEvidenceArtifact, String>;
type LiveEvidenceLines = fn(&Path, &Path, Option<&Path>, &Path) -> Result<Vec<String>, String>;

#[derive(Clone, Copy)]
struct HermeticRunner {
    run_and_print: HermeticRunAndPrint,
    evidence_lines: HermeticEvidenceLines,
}

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
    hermetic: Option<HermeticRunner>,
    live: Option<LiveRunner>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RunnerSelector {
    RunnerUses,
    LegacyName,
}

#[derive(Clone, Copy)]
pub(super) struct SelectedRunner {
    runner: &'static RunnerDefinition,
    selector: RunnerSelector,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(super) enum RunnerRegistryError {
    UnsupportedRunner {
        requested: String,
        selector: RunnerSelector,
    },
    UnsupportedTier {
        requested: String,
        selector: RunnerSelector,
        runner_id: &'static str,
        tier: ScenarioTier,
        supported_tiers: String,
    },
}

static RUNNERS: &[RunnerDefinition] = &[
    RunnerDefinition {
        id: basic_delivery::SCENARIO_NAME,
        supported_tiers: &[ScenarioTier::Hermetic, ScenarioTier::Live],
        hermetic: Some(HermeticRunner {
            run_and_print: basic_delivery::run_and_print,
            evidence_lines: basic_delivery::run_evidence_lines,
        }),
        live: Some(LiveRunner {
            run_and_print: basic_delivery::run_live_and_print,
            evidence_lines: basic_delivery::run_live_evidence_lines_for_report,
            requires_standalone_temper: true,
        }),
    },
    RunnerDefinition {
        id: implementation_pr_handoff::SCENARIO_NAME,
        supported_tiers: &[ScenarioTier::Hermetic],
        hermetic: Some(HermeticRunner {
            run_and_print: implementation_pr_handoff::run_and_print,
            evidence_lines: implementation_pr_handoff::run_evidence_lines,
        }),
        live: None,
    },
];

pub(super) fn select_runner(
    manifest: &ScenarioManifest,
    tier: ScenarioTier,
) -> Result<SelectedRunner, RunnerRegistryError> {
    let (selector, requested) = requested_runner(manifest);
    let Some(runner) = RUNNERS.iter().find(|runner| runner.id == requested) else {
        return Err(RunnerRegistryError::UnsupportedRunner {
            requested: requested.to_string(),
            selector,
        });
    };
    if !runner.supports_tier(tier) {
        return Err(RunnerRegistryError::UnsupportedTier {
            requested: requested.to_string(),
            selector,
            runner_id: runner.id,
            tier,
            supported_tiers: runner.supported_tiers_display(),
        });
    }
    Ok(SelectedRunner { runner, selector })
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

fn requested_runner(manifest: &ScenarioManifest) -> (RunnerSelector, &str) {
    if let Some(uses) = manifest.runner.uses.as_deref() {
        (RunnerSelector::RunnerUses, uses)
    } else {
        (RunnerSelector::LegacyName, manifest.name.as_str())
    }
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
        format!(
            "runner: `{}` selected by {}",
            self.runner.id(),
            self.selector.description()
        )
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
        match tier {
            ScenarioTier::Hermetic => {
                let runner = self
                    .runner
                    .hermetic
                    .ok_or_else(|| self.invariant_error(tier))?;
                (runner.run_and_print)(scenario_path, manifest_path, facts, evidence_context)
            }
            ScenarioTier::Live => {
                let runner = self.runner.live.ok_or_else(|| self.invariant_error(tier))?;
                (runner.run_and_print)(
                    scenario_path,
                    manifest_path,
                    facts,
                    temper_bin,
                    evidence_context,
                )
            }
        }
    }

    pub(super) fn hermetic_evidence_lines(
        &self,
        scenario_path: &Path,
        manifest_path: &Path,
    ) -> Result<Vec<String>, String> {
        let runner = self
            .runner
            .hermetic
            .ok_or_else(|| self.invariant_error(ScenarioTier::Hermetic))?;
        (runner.evidence_lines)(scenario_path, manifest_path)
    }

    pub(super) fn live_evidence_lines(
        &self,
        scenario_path: &Path,
        manifest_path: &Path,
        temper_bin: Option<&Path>,
        artifact_dir: &Path,
    ) -> Result<Vec<String>, String> {
        let runner = self
            .runner
            .live
            .ok_or_else(|| self.invariant_error(ScenarioTier::Live))?;
        (runner.evidence_lines)(scenario_path, manifest_path, temper_bin, artifact_dir)
    }

    pub(super) fn selector_key(&self) -> &'static str {
        self.selector.key()
    }

    pub(super) fn requires_standalone_temper(&self, tier: ScenarioTier) -> bool {
        match tier {
            ScenarioTier::Hermetic => false,
            ScenarioTier::Live => self
                .runner
                .live
                .is_some_and(|runner| runner.requires_standalone_temper),
        }
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
            Self::UnsupportedRunner {
                requested,
                selector,
            } => unsupported_runner_message(requested, *selector, scenario_path),
            Self::UnsupportedTier {
                requested,
                selector,
                runner_id,
                tier,
                supported_tiers,
            } => format!(
                "unsupported tier `{}` for runner `{runner_id}` selected by {} at {}; requested runner `{requested}`; supported tiers: {supported_tiers}; supported runner ids: {}; refusing to substitute another runner",
                tier.as_str(),
                selector.description(),
                scenario_path.display(),
                supported_runners_display()
            ),
        }
    }

    pub(super) fn is_unsupported_runner(&self) -> bool {
        matches!(self, Self::UnsupportedRunner { .. })
    }
}

impl RunnerSelector {
    fn key(self) -> &'static str {
        match self {
            Self::RunnerUses => "runner.uses",
            Self::LegacyName => "legacy_name",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::RunnerUses => "runner.uses",
            Self::LegacyName => "legacy scenario name fallback",
        }
    }
}

fn unsupported_runner_message(
    requested: &str,
    selector: RunnerSelector,
    scenario_path: &Path,
) -> String {
    match selector {
        RunnerSelector::RunnerUses => format!(
            "unsupported runner `{requested}` selected by runner.uses at {}; supported runner ids: {}",
            scenario_path.display(),
            supported_runners_display()
        ),
        RunnerSelector::LegacyName => format!(
            "unsupported scenario `{requested}`: unsupported runner `{requested}` selected by legacy scenario name fallback at {}; supported runner ids: {}",
            scenario_path.display(),
            supported_runners_display()
        ),
    }
}
