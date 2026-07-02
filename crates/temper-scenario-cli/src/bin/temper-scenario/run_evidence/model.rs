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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) assertions: Option<AssertionEvidence>,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) repositories: Vec<RepositoryStateEvidence>,
    #[serde(default)]
    pub(crate) ci: CiStateEvidence,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct IssueStateEvidence {
    pub(crate) number: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) id: Option<String>,
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
    pub(crate) id: Option<String>,
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
pub(crate) struct RepositoryStateEvidence {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) slug: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) branches: Vec<RepositoryBranchStateEvidence>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct RepositoryBranchStateEvidence {
    pub(crate) name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) head_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) contains_engineer_diff: Option<bool>,
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
    pub(crate) pull_request_number: Option<u64>,
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
pub(crate) struct AssertionEvidence {
    pub(crate) status: String,
    pub(crate) total: usize,
    pub(crate) passed: usize,
    pub(crate) failed: usize,
    pub(crate) unsupported: usize,
    #[serde(default)]
    pub(crate) results: Vec<AssertionResultEvidence>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct AssertionResultEvidence {
    pub(crate) id: String,
    pub(crate) status: String,
    pub(crate) description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) artifact: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) phase: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) context_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) stdout_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) stderr_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) status_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) exit_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) details: Vec<String>,
}

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct ArtifactCollections {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) log_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) artifact_paths: Vec<String>,
}

impl AssertionEvidence {
    pub(crate) fn from_results(results: Vec<AssertionResultEvidence>) -> Self {
        let passed = results
            .iter()
            .filter(|result| result.status == ASSERTION_STATUS_PASSED)
            .count();
        let failed = results
            .iter()
            .filter(|result| result.status == ASSERTION_STATUS_FAILED)
            .count();
        let unsupported = results
            .iter()
            .filter(|result| result.status == ASSERTION_STATUS_UNSUPPORTED)
            .count();
        let status = if failed == 0 {
            ASSERTION_STATUS_PASSED
        } else {
            ASSERTION_STATUS_FAILED
        };
        Self {
            status: status.to_string(),
            total: results.len(),
            passed,
            failed,
            unsupported,
            results,
        }
    }

    pub(crate) fn append_result(&mut self, result: AssertionResultEvidence) {
        let mut results = self.results.clone();
        results.push(result);
        *self = Self::from_results(results);
    }

    pub(crate) fn has_failures(&self) -> bool {
        self.failed > 0
            || self
                .results
                .iter()
                .any(|result| result.status == ASSERTION_STATUS_FAILED)
    }

    pub(crate) fn summary(&self) -> String {
        format!(
            "{} ({} passed, {} failed, {} unsupported)",
            self.status, self.passed, self.failed, self.unsupported
        )
    }

    pub(crate) fn report_details(&self) -> Vec<String> {
        let mut details = vec![format!("manifest assertions: {}", self.summary())];
        for result in &self.results {
            let mut line = format!(
                "assertion {} `{}`: {}",
                result.status, result.id, result.description
            );
            if let Some(kind) = result.kind.as_deref() {
                line.push_str(&format!(" kind={kind}"));
            }
            if let Some(phase) = result.phase.as_deref() {
                line.push_str(&format!(" phase={phase}"));
            }
            if let Some(artifact) = result.artifact.as_deref() {
                line.push_str(&format!(" ({artifact})"));
            }
            details.push(line);
            for detail in &result.details {
                details.push(format!("assertion `{}` detail: {detail}", result.id));
            }
        }
        details
    }
}

pub(crate) const ASSERTION_STATUS_PASSED: &str = "passed";
pub(crate) const ASSERTION_STATUS_FAILED: &str = "failed";
pub(crate) const ASSERTION_STATUS_UNSUPPORTED: &str = "unsupported";

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
