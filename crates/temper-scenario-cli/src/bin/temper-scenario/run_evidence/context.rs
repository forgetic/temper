// SPDX-License-Identifier: MPL-2.0

use temper_scenario_core::{CheckReport, ScenarioTopology};

use crate::run_context::ScenarioRunFacts;
use crate::runner_registry::SelectedRunner;

use super::model::{
    ArtifactCollections, FinalStateEvidence, FixtureEvidence, RUN_EVIDENCE_SCHEMA,
    RUN_EVIDENCE_VERSION, RunEvidenceArtifact, ScenarioEvidence, TopologyEvidence,
};

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct RunEvidenceContext {
    pub(crate) scenario: ScenarioEvidence,
    pub(crate) fixtures: Vec<FixtureEvidence>,
}

impl RunEvidenceContext {
    pub(crate) fn from_check_report(
        check_report: &CheckReport,
        facts: &ScenarioRunFacts,
        selected_runner: &SelectedRunner,
    ) -> Self {
        let manifest = check_report.manifest.as_ref();
        let scenario_name = manifest
            .map(|manifest| manifest.name.clone())
            .unwrap_or_else(|| "unknown".to_string());
        let manifest_path = check_report
            .manifest_path
            .as_deref()
            .unwrap_or(check_report.scenario_path.as_path());
        let fixtures = manifest
            .map(|manifest| {
                manifest
                    .path_references
                    .iter()
                    .map(|reference| FixtureEvidence {
                        field: reference.field.clone(),
                        value: reference.value.clone(),
                        resolved_path: reference.resolved_path.display().to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            scenario: ScenarioEvidence {
                name: scenario_name,
                source: facts.source.evidence_value().to_string(),
                source_description: facts.source.as_str().to_string(),
                scenario_path: check_report.scenario_path.display().to_string(),
                manifest_path: manifest_path.display().to_string(),
                runner_id: selected_runner.id().to_string(),
                runner_selector: selected_runner.selector_key().to_string(),
                runner_selection: selected_runner.selection_detail(),
                tier: facts.tier.as_str().to_string(),
                tier_description: facts.tier.description().to_string(),
                topology: TopologyEvidence::from_topology(&facts.topology),
            },
            fixtures,
        }
    }

    pub(crate) fn artifact(&self, final_state: FinalStateEvidence) -> RunEvidenceArtifact {
        RunEvidenceArtifact {
            schema: RUN_EVIDENCE_SCHEMA.to_string(),
            version: RUN_EVIDENCE_VERSION,
            scenario: self.scenario.clone(),
            fixtures: self.fixtures.clone(),
            final_state,
            convergence: None,
            provider: None,
            artifacts: ArtifactCollections::default(),
            evidence_lines: Vec::new(),
        }
    }
}

impl TopologyEvidence {
    fn from_topology(topology: &ScenarioTopology) -> Self {
        Self {
            kind: topology.kind.clone(),
            forge: topology.forge.clone(),
            runner: topology.runner.clone(),
            temper: topology.temper.clone(),
            agent_model: topology.agent_model.clone(),
        }
    }
}
