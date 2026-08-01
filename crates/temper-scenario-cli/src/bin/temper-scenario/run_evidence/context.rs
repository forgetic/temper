// SPDX-License-Identifier: MPL-2.0

use std::path::Path;
use std::process::Command;

use temper_scenario_core::{CheckReport, ScenarioTopology, scenario_content_digest};

use crate::run_context::{LIVE_TIER, LIVE_TOPOLOGY_DESCRIPTION, ScenarioRunFacts};
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
        let identity = scenario_identity(manifest_path, check_report);
        Self {
            scenario: ScenarioEvidence {
                name: scenario_name.clone(),
                source: facts.source.evidence_value().to_string(),
                source_description: facts.source.as_str().to_string(),
                scenario_path: check_report.scenario_path.display().to_string(),
                manifest_path: manifest_path.display().to_string(),
                feature: identity.feature,
                plan: identity.plan,
                mapping_id: identity.mapping_id,
                mapped_scenario: Some(scenario_name),
                source_branch: identity.source_branch,
                checkout_head_sha: identity.checkout_head_sha,
                resolved_content_digest: identity.resolved_content_digest,
                runner_id: selected_runner.id().to_string(),
                runner_selector: selected_runner.selector_key().to_string(),
                runner_selection: selected_runner.selection_detail(),
                tier: LIVE_TIER.to_string(),
                tier_description: LIVE_TOPOLOGY_DESCRIPTION.to_string(),
                topology: TopologyEvidence::from_topology(&facts.topology),
            },
            fixtures,
        }
    }

    pub(crate) fn artifact(&self, final_state: FinalStateEvidence) -> RunEvidenceArtifact {
        RunEvidenceArtifact {
            schema: RUN_EVIDENCE_SCHEMA.to_string(),
            version: RUN_EVIDENCE_VERSION,
            verdict: super::model::RunEvidenceVerdict::Passed,
            scenario: self.scenario.clone(),
            binary: None,
            execution: None,
            fixtures: self.fixtures.clone(),
            final_state,
            convergence: None,
            provider: None,
            observability: None,
            artifacts: ArtifactCollections::default(),
            evidence_lines: Vec::new(),
            stimuli: Vec::new(),
            limitations: Vec::new(),
            follow_up_intent: None,
            assertions: None,
        }
    }

    pub(crate) fn failure_artifact(
        &self,
        message: impl Into<String>,
        total_duration_ms: u64,
    ) -> RunEvidenceArtifact {
        let message = message.into();
        let mut artifact = self.artifact(FinalStateEvidence::default());
        artifact.verdict = super::model::RunEvidenceVerdict::Failed;
        artifact.execution = Some(super::model::ExecutionEvidence {
            status: "failed".to_string(),
            total_duration_ms,
            failure: Some(message.clone()),
        });
        artifact.limitations.push(
            "The live executor failed before complete final-state collection; retained diagnostics describe the last observable state."
                .to_string(),
        );
        artifact.follow_up_intent = Some(
            "Repair the live execution failure and rerun this exact scenario at the same checkout head."
                .to_string(),
        );
        artifact
            .evidence_lines
            .push(format!("execution failure: {message}"));
        for path in retained_failure_paths(&message) {
            if path.is_dir() {
                push_unique_path(
                    &mut artifact.artifacts.artifact_paths,
                    path.display().to_string(),
                );
            } else {
                push_unique_path(
                    &mut artifact.artifacts.log_paths,
                    path.display().to_string(),
                );
                if let Some(workspace) = path.parent().and_then(Path::parent) {
                    push_unique_path(
                        &mut artifact.artifacts.artifact_paths,
                        workspace.display().to_string(),
                    );
                }
            }
        }
        artifact
    }
}

fn retained_failure_paths(message: &str) -> Vec<std::path::PathBuf> {
    message
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            let raw = trimmed
                .strip_prefix("retained workspace: ")
                .or_else(|| trimmed.strip_prefix("standalone log: "))
                .or_else(|| trimmed.strip_prefix("CI diagnostics: "))
                .or_else(|| {
                    trimmed
                        .strip_prefix("--- ")
                        .and_then(|line| line.strip_suffix(" ---"))
                        .and_then(|line| line.rsplit_once('('))
                        .map(|(_, path)| path.trim_end_matches(')'))
                })?;
            let path = std::path::PathBuf::from(raw);
            path.exists().then_some(path)
        })
        .collect()
}

fn push_unique_path(paths: &mut Vec<String>, path: String) {
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

#[derive(Default)]
struct ScenarioIdentity {
    feature: Option<String>,
    plan: Option<String>,
    mapping_id: Option<String>,
    source_branch: Option<String>,
    checkout_head_sha: Option<String>,
    resolved_content_digest: Option<String>,
}

fn scenario_identity(manifest_path: &Path, check_report: &CheckReport) -> ScenarioIdentity {
    let mapping = check_report
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.feature_mapping.as_ref());
    let feature = mapping.map(|mapping| mapping.feature.to_string());
    let plan = mapping
        .and_then(|mapping| mapping.plan.as_ref())
        .map(ToString::to_string);
    let mapping_id = mapping.and_then(|mapping| {
        check_report
            .manifest
            .as_ref()
            .map(|manifest| mapping.identity(&manifest.name))
    });
    let checkout = find_checkout_root(manifest_path);
    ScenarioIdentity {
        feature,
        plan,
        mapping_id,
        source_branch: mapping
            .map(|mapping| mapping.source_branch.clone())
            .or_else(|| {
                checkout
                    .as_deref()
                    .and_then(|root| git_output(root, &["rev-parse", "--abbrev-ref", "HEAD"]))
            }),
        checkout_head_sha: checkout
            .as_deref()
            .and_then(|root| git_output(root, &["rev-parse", "HEAD"])),
        resolved_content_digest: scenario_content_digest(check_report).ok(),
    }
}

fn find_checkout_root(path: &Path) -> Option<std::path::PathBuf> {
    let mut current = if path.is_file() { path.parent()? } else { path };
    loop {
        if current.join(".git").exists() {
            return Some(current.to_path_buf());
        }
        current = current.parent()?;
    }
}

fn git_output(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
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
