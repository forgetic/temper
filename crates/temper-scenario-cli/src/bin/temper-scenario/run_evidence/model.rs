// SPDX-License-Identifier: MPL-2.0

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub(crate) const RUN_EVIDENCE_SCHEMA: &str = "temper.scenario.run-evidence";
pub(crate) const RUN_EVIDENCE_VERSION: u64 = 1;
pub(crate) const DEFAULT_RUN_EVIDENCE_FILE: &str = "run-evidence.json";

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct RunEvidenceArtifact {
    pub(crate) schema: String,
    pub(crate) version: u64,
    pub(crate) scenario: ScenarioEvidence,
    #[serde(default)]
    pub(crate) fixtures: Vec<FixtureEvidence>,
    pub(crate) final_state: FinalStateEvidence,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) convergence: Option<ConvergenceEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) provider: Option<ProviderEvidence>,
    #[serde(default, skip_serializing_if = "ArtifactCollections::is_empty")]
    pub(crate) artifacts: ArtifactCollections,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) evidence_lines: Vec<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct ScenarioEvidence {
    pub(crate) name: String,
    pub(crate) source: String,
    pub(crate) source_description: String,
    pub(crate) scenario_path: String,
    pub(crate) manifest_path: String,
    pub(crate) runner_id: String,
    pub(crate) runner_selector: String,
    pub(crate) runner_selection: String,
    pub(crate) tier: String,
    pub(crate) tier_description: String,
    pub(crate) topology: TopologyEvidence,
}

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct TopologyEvidence {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) forge: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) runner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) temper: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) agent_model: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct FixtureEvidence {
    pub(crate) field: String,
    pub(crate) value: String,
    pub(crate) resolved_path: String,
}

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct FinalStateEvidence {
    #[serde(default)]
    pub(crate) issues: Vec<IssueStateEvidence>,
    #[serde(default)]
    pub(crate) pull_requests: Vec<PullRequestStateEvidence>,
    #[serde(default)]
    pub(crate) ci: CiStateEvidence,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct IssueStateEvidence {
    pub(crate) number: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) state: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) labels: Vec<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct PullRequestStateEvidence {
    pub(crate) number: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) state: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) head_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) head_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) merged_sha: Option<String>,
}

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct CiStateEvidence {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) completed_jobs: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) jobs: Vec<CiJobEvidence>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct CiJobEvidence {
    pub(crate) name: String,
    pub(crate) status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) conclusion: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) url: Option<String>,
}

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct ConvergenceEvidence {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) ticks: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) workers: Vec<WorkerTickEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) startup_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) convergence_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) total_elapsed_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) poll_backstop_ms: Option<u64>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct WorkerTickEvidence {
    pub(crate) name: String,
    pub(crate) ticks: u64,
    pub(crate) actions: u64,
}

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct ProviderEvidence {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) forgejo_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) repo_slug: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) issue_number: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pr_number: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) head_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) merged_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) temper_binary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) fake_llm_url: Option<String>,
}

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct ArtifactCollections {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) log_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) artifact_paths: Vec<String>,
}

impl ArtifactCollections {
    fn is_empty(&self) -> bool {
        self.log_paths.is_empty() && self.artifact_paths.is_empty()
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct LoadedRunEvidence {
    pub(crate) path: PathBuf,
    pub(crate) artifact: RunEvidenceArtifact,
}
