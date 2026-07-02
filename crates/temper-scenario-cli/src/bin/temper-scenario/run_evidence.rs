// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use temper_scenario_core::{CheckReport, ScenarioTopology};

use super::run_context::{ScenarioRunFacts, ScenarioTier};
use super::runner_registry::SelectedRunner;

pub(super) const RUN_EVIDENCE_SCHEMA: &str = "temper.scenario.run-evidence";
pub(super) const RUN_EVIDENCE_VERSION: u64 = 1;
pub(super) const DEFAULT_RUN_EVIDENCE_FILE: &str = "run-evidence.json";

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(super) struct RunEvidenceArtifact {
    pub schema: String,
    pub version: u64,
    pub scenario: ScenarioEvidence,
    #[serde(default)]
    pub fixtures: Vec<FixtureEvidence>,
    pub final_state: FinalStateEvidence,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub convergence: Option<ConvergenceEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderEvidence>,
    #[serde(default, skip_serializing_if = "ArtifactCollections::is_empty")]
    pub artifacts: ArtifactCollections,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_lines: Vec<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(super) struct ScenarioEvidence {
    pub name: String,
    pub source: String,
    pub source_description: String,
    pub scenario_path: String,
    pub manifest_path: String,
    pub runner_id: String,
    pub runner_selector: String,
    pub runner_selection: String,
    pub tier: String,
    pub tier_description: String,
    pub topology: TopologyEvidence,
}

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
pub(super) struct TopologyEvidence {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forge: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temper: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_model: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(super) struct FixtureEvidence {
    pub field: String,
    pub value: String,
    pub resolved_path: String,
}

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
pub(super) struct FinalStateEvidence {
    #[serde(default)]
    pub issues: Vec<IssueStateEvidence>,
    #[serde(default)]
    pub pull_requests: Vec<PullRequestStateEvidence>,
    #[serde(default)]
    pub ci: CiStateEvidence,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(super) struct IssueStateEvidence {
    pub number: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(super) struct PullRequestStateEvidence {
    pub number: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged_sha: Option<String>,
}

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
pub(super) struct CiStateEvidence {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_jobs: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jobs: Vec<CiJobEvidence>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(super) struct CiJobEvidence {
    pub name: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conclusion: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
pub(super) struct ConvergenceEvidence {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticks: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workers: Vec<WorkerTickEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub convergence_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_elapsed_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poll_backstop_ms: Option<u64>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(super) struct WorkerTickEvidence {
    pub name: String,
    pub ticks: u64,
    pub actions: u64,
}

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
pub(super) struct ProviderEvidence {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forgejo_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_slug: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_number: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_number: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temper_binary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fake_llm_url: Option<String>,
}

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
pub(super) struct ArtifactCollections {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub log_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_paths: Vec<String>,
}

impl ArtifactCollections {
    fn is_empty(&self) -> bool {
        self.log_paths.is_empty() && self.artifact_paths.is_empty()
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(super) struct RunEvidenceContext {
    pub scenario: ScenarioEvidence,
    pub fixtures: Vec<FixtureEvidence>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(super) struct LoadedRunEvidence {
    pub path: PathBuf,
    pub artifact: RunEvidenceArtifact,
}

impl RunEvidenceContext {
    pub(super) fn from_check_report(
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

    pub(super) fn artifact(&self, final_state: FinalStateEvidence) -> RunEvidenceArtifact {
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

    fn is_empty(&self) -> bool {
        self.kind.is_none()
            && self.forge.is_none()
            && self.runner.is_none()
            && self.temper.is_none()
            && self.agent_model.is_none()
    }

    fn field_values(&self) -> Vec<(&'static str, &str)> {
        [
            ("kind", self.kind.as_deref()),
            ("forge", self.forge.as_deref()),
            ("runner", self.runner.as_deref()),
            ("temper", self.temper.as_deref()),
            ("agent_model", self.agent_model.as_deref()),
        ]
        .into_iter()
        .filter_map(|(name, value)| value.map(|value| (name, value)))
        .collect()
    }
}

impl RunEvidenceArtifact {
    pub(super) fn validate(&self) -> Vec<String> {
        let mut diagnostics = Vec::new();
        if self.schema != RUN_EVIDENCE_SCHEMA {
            diagnostics.push(format!(
                "run evidence schema mismatch: expected `{RUN_EVIDENCE_SCHEMA}`, got `{}`",
                self.schema
            ));
        }
        if self.version != RUN_EVIDENCE_VERSION {
            diagnostics.push(format!(
                "run evidence version mismatch: expected {RUN_EVIDENCE_VERSION}, got {}",
                self.version
            ));
        }
        if self.scenario.name.trim().is_empty() {
            diagnostics.push("run evidence scenario.name is missing".to_string());
        }
        if self.scenario.manifest_path.trim().is_empty() {
            diagnostics.push("run evidence scenario.manifest_path is missing".to_string());
        }
        if !matches!(self.scenario.source.as_str(), "checked_in" | "ephemeral") {
            diagnostics.push(format!(
                "run evidence scenario.source must be `checked_in` or `ephemeral`, got `{}`",
                self.scenario.source
            ));
        }
        if ScenarioTier::parse(&self.scenario.tier).is_none() {
            diagnostics.push(format!(
                "run evidence scenario.tier must be `hermetic` or `live`, got `{}`",
                self.scenario.tier
            ));
        }
        if self.scenario.runner_id.trim().is_empty() {
            diagnostics.push("run evidence scenario.runner_id is missing".to_string());
        }
        if self.final_state.issues.is_empty()
            && self.final_state.pull_requests.is_empty()
            && self.final_state.ci.completed_jobs.is_none()
            && self.final_state.ci.jobs.is_empty()
        {
            diagnostics.push(
                "run evidence final_state has no issue, pull request, or CI data".to_string(),
            );
        }
        diagnostics
    }

    pub(super) fn report_details(&self, path: &Path) -> Vec<String> {
        let mut details = vec![
            format!("run evidence artifact: `{}`", path.display()),
            format!("schema: `{}` version {}", self.schema, self.version),
            format!("scenario: `{}`", self.scenario.name),
            format!("source: {}", self.scenario.source_description),
            format!("manifest: `{}`", self.scenario.manifest_path),
            format!(
                "confidence tier: {} ({})",
                self.scenario.tier, self.scenario.tier_description
            ),
            self.scenario.runner_selection.clone(),
        ];
        if self.scenario.topology.is_empty() {
            details.push("manifest topology: not declared".to_string());
        } else {
            details.extend(
                self.scenario
                    .topology
                    .field_values()
                    .into_iter()
                    .map(|(field, value)| format!("manifest topology.{field}: `{value}`")),
            );
        }
        for fixture in &self.fixtures {
            details.push(format!(
                "fixture {}: `{}` -> `{}`",
                fixture.field, fixture.value, fixture.resolved_path
            ));
        }
        details.extend(final_state_details(&self.final_state));
        if let Some(convergence) = &self.convergence {
            details.extend(convergence_details(convergence));
        }
        if let Some(provider) = &self.provider {
            details.extend(provider_details(provider));
        }
        for path in &self.artifacts.log_paths {
            details.push(format!("log path: `{path}`"));
        }
        for path in &self.artifacts.artifact_paths {
            details.push(format!("artifact path: `{path}`"));
        }
        for line in &self.evidence_lines {
            details.push(format!("runner evidence: {line}"));
        }
        details
    }

    pub(super) fn write_to_path(&self, path: &Path) -> Result<PathBuf, String> {
        let output_path = if path.is_dir() {
            path.join(DEFAULT_RUN_EVIDENCE_FILE)
        } else {
            path.to_path_buf()
        };
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "create run evidence output directory {}: {error}",
                    parent.display()
                )
            })?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|error| format!("serialize run evidence artifact: {error}"))?;
        fs::write(&output_path, format!("{json}\n")).map_err(|error| {
            format!(
                "write run evidence artifact {}: {error}",
                output_path.display()
            )
        })?;
        Ok(output_path)
    }
}

pub(super) fn load_run_evidence(path: &Path) -> Result<LoadedRunEvidence, String> {
    let path = resolve_run_evidence_path(path)?;
    let source = fs::read_to_string(&path)
        .map_err(|error| format!("read run evidence artifact {}: {error}", path.display()))?;
    let artifact = serde_json::from_str::<RunEvidenceArtifact>(&source)
        .map_err(|error| format!("parse run evidence artifact {}: {error}", path.display()))?;
    Ok(LoadedRunEvidence { path, artifact })
}

fn resolve_run_evidence_path(path: &Path) -> Result<PathBuf, String> {
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    if path.is_dir() {
        let default = path.join(DEFAULT_RUN_EVIDENCE_FILE);
        if default.is_file() {
            return Ok(default);
        }
        let mut candidates = Vec::new();
        for entry in fs::read_dir(path)
            .map_err(|error| format!("read run evidence directory {}: {error}", path.display()))?
        {
            let entry = entry.map_err(|error| {
                format!(
                    "read run evidence directory entry {}: {error}",
                    path.display()
                )
            })?;
            let candidate = entry.path();
            if candidate.is_file()
                && candidate
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(".run-evidence.json"))
            {
                candidates.push(candidate);
            }
        }
        candidates.sort();
        return match candidates.as_slice() {
            [candidate] => Ok(candidate.clone()),
            [] => Err(format!(
                "run evidence directory {} does not contain {DEFAULT_RUN_EVIDENCE_FILE} or a *.run-evidence.json file",
                path.display()
            )),
            _ => Err(format!(
                "run evidence directory {} contains multiple *.run-evidence.json files; pass one file path explicitly",
                path.display()
            )),
        };
    }
    Err(format!(
        "run evidence path does not exist or is not readable: {}",
        path.display()
    ))
}

fn final_state_details(final_state: &FinalStateEvidence) -> Vec<String> {
    let mut details = Vec::new();
    for issue in &final_state.issues {
        let state = issue.state.as_deref().unwrap_or("unknown");
        let title = issue.title.as_deref().unwrap_or("untitled");
        details.push(format!(
            "final issue: #{} `{}` state={} labels={:?}",
            issue.number, title, state, issue.labels
        ));
    }
    for pull_request in &final_state.pull_requests {
        let state = pull_request.state.as_deref().unwrap_or("unknown");
        let title = pull_request.title.as_deref().unwrap_or("untitled");
        let mut detail = format!(
            "final PR: #{} `{}` state={} labels={:?}",
            pull_request.number, title, state, pull_request.labels
        );
        if let Some(head_branch) = pull_request.head_branch.as_deref() {
            detail.push_str(&format!(" head={head_branch}"));
        }
        if let Some(head_sha) = pull_request.head_sha.as_deref() {
            detail.push_str(&format!(" head_sha={head_sha}"));
        }
        if let Some(merged_sha) = pull_request.merged_sha.as_deref() {
            detail.push_str(&format!(" merged_sha={merged_sha}"));
        }
        details.push(detail);
    }
    if let Some(completed_jobs) = final_state.ci.completed_jobs {
        details.push(format!("final CI: {completed_jobs} completed job(s)"));
    }
    for job in &final_state.ci.jobs {
        details.push(format!(
            "final CI job: name={} status={} conclusion={:?} url={:?}",
            job.name, job.status, job.conclusion, job.url
        ));
    }
    details
}

fn convergence_details(convergence: &ConvergenceEvidence) -> Vec<String> {
    let mut details = Vec::new();
    if let Some(ticks) = convergence.ticks {
        details.push(format!("convergence ticks: {ticks}"));
    }
    for worker in &convergence.workers {
        details.push(format!(
            "convergence worker: {} ticks={} actions={}",
            worker.name, worker.ticks, worker.actions
        ));
    }
    for (field, value) in [
        ("startup_ms", convergence.startup_ms),
        ("convergence_ms", convergence.convergence_ms),
        ("poll_backstop_ms", convergence.poll_backstop_ms),
        ("total_elapsed_ms", convergence.total_elapsed_ms),
    ] {
        if let Some(value) = value {
            details.push(format!("convergence {field}: {value}"));
        }
    }
    details
}

fn provider_details(provider: &ProviderEvidence) -> Vec<String> {
    let mut details = Vec::new();
    for (field, value) in [
        ("forgejo_url", provider.forgejo_url.as_deref()),
        ("repo_slug", provider.repo_slug.as_deref()),
        ("head_branch", provider.head_branch.as_deref()),
        ("merged_sha", provider.merged_sha.as_deref()),
        ("temper_binary", provider.temper_binary.as_deref()),
        ("fake_llm_url", provider.fake_llm_url.as_deref()),
    ] {
        if let Some(value) = value {
            details.push(format!("provider {field}: `{value}`"));
        }
    }
    if let Some(issue_number) = provider.issue_number {
        details.push(format!("provider issue_number: #{issue_number}"));
    }
    if let Some(pr_number) = provider.pr_number {
        details.push(format!("provider pr_number: #{pr_number}"));
    }
    details
}
