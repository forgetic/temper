// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub(crate) const RUN_EVIDENCE_SCHEMA: &str = "temper.scenario.run-evidence";
pub(crate) const RUN_EVIDENCE_VERSION: u64 = 2;
pub(crate) const LEGACY_RUN_EVIDENCE_VERSION: u64 = 1;
pub(crate) const DEFAULT_RUN_EVIDENCE_FILE: &str = "run-evidence.json";

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct RunEvidenceArtifact {
    pub(crate) schema: String,
    pub(crate) version: u64,
    #[serde(default)]
    pub(crate) verdict: RunEvidenceVerdict,
    pub(crate) scenario: ScenarioEvidence,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) binary: Option<BinaryIdentityEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) execution: Option<ExecutionEvidence>,
    #[serde(default)]
    pub(crate) fixtures: Vec<FixtureEvidence>,
    pub(crate) final_state: FinalStateEvidence,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) convergence: Option<ConvergenceEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) provider: Option<ProviderEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) observability: Option<ObservabilityEvidence>,
    #[serde(default, skip_serializing_if = "ArtifactCollections::is_empty")]
    pub(crate) artifacts: ArtifactCollections,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) evidence_lines: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) stimuli: Vec<StimulusEvidence>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) limitations: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) follow_up_intent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) assertions: Option<AssertionEvidence>,
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RunEvidenceVerdict {
    #[default]
    Passed,
    Failed,
    Inconclusive,
}

impl RunEvidenceVerdict {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Inconclusive => "inconclusive",
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct BinaryIdentityEvidence {
    pub(crate) path: String,
    pub(crate) sha256: String,
    pub(crate) size_bytes: u64,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct ExecutionEvidence {
    pub(crate) status: String,
    pub(crate) total_duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) failure: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct StimulusEvidence {
    pub(crate) id: String,
    pub(crate) action: String,
    pub(crate) status: String,
    pub(crate) attempts: u64,
    pub(crate) timeout_ms: u64,
    pub(crate) duration_ms: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) details: Vec<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct ScenarioEvidence {
    pub(crate) name: String,
    pub(crate) source: String,
    pub(crate) source_description: String,
    pub(crate) scenario_path: String,
    pub(crate) manifest_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) feature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) plan: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) mapping_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) mapped_scenario: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) source_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) checkout_head_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) resolved_content_digest: Option<String>,
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
    pub(crate) body: Option<String>,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) jig_script_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) request_log_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) request_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) request_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) request_counts_by_role: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct ObservabilityEvidence {
    pub(crate) scenario_run_id: String,
    pub(crate) log_format: String,
    pub(crate) rust_log: String,
    pub(crate) event_log_path: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) event_log_paths: Vec<String>,
    pub(crate) captured_events: usize,
    #[serde(default)]
    pub(crate) events: Vec<StructuredEventEvidence>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct StructuredEventEvidence {
    pub(crate) sequence: usize,
    pub(crate) event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) service: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) target: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) fields: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct AssertionEvidence {
    pub(crate) status: String,
    pub(crate) total: usize,
    #[serde(default)]
    pub(crate) required: usize,
    pub(crate) passed: usize,
    pub(crate) failed: usize,
    #[serde(default)]
    pub(crate) missing_fact: usize,
    #[serde(default)]
    pub(crate) timed_out: usize,
    pub(crate) unsupported: usize,
    #[serde(default)]
    pub(crate) blocked_required: usize,
    #[serde(default)]
    pub(crate) results: Vec<AssertionResultEvidence>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct AssertionResultEvidence {
    pub(crate) id: String,
    #[serde(default = "required_by_default")]
    pub(crate) required: bool,
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
        let count = |status: &str| {
            results
                .iter()
                .filter(|result| result.status == status)
                .count()
        };
        let required = results.iter().filter(|result| result.required).count();
        let passed = count(ASSERTION_STATUS_PASSED);
        let failed = count(ASSERTION_STATUS_FAILED);
        let missing_fact = count(ASSERTION_STATUS_MISSING_FACT);
        let timed_out = count(ASSERTION_STATUS_TIMED_OUT);
        let unsupported = count(ASSERTION_STATUS_UNSUPPORTED);
        let blocked_required = results
            .iter()
            .filter(|result| result.required && result.status != ASSERTION_STATUS_PASSED)
            .count();
        let has_required_failure = results.iter().any(|result| {
            result.required
                && matches!(
                    result.status.as_str(),
                    ASSERTION_STATUS_FAILED | ASSERTION_STATUS_TIMED_OUT
                )
        });
        let status = if has_required_failure {
            ASSERTION_STATUS_FAILED
        } else if blocked_required > 0 {
            ASSERTION_STATUS_INCONCLUSIVE
        } else {
            ASSERTION_STATUS_PASSED
        };
        Self {
            status: status.to_string(),
            total: results.len(),
            required,
            passed,
            failed,
            missing_fact,
            timed_out,
            unsupported,
            blocked_required,
            results,
        }
    }

    pub(crate) fn append_result(&mut self, result: AssertionResultEvidence) {
        let mut results = self.results.clone();
        results.push(result);
        *self = Self::from_results(results);
    }

    pub(crate) fn has_failures(&self) -> bool {
        self.blocked_required > 0
    }

    pub(crate) fn verdict(&self) -> RunEvidenceVerdict {
        match self.status.as_str() {
            ASSERTION_STATUS_FAILED => RunEvidenceVerdict::Failed,
            ASSERTION_STATUS_INCONCLUSIVE => RunEvidenceVerdict::Inconclusive,
            _ => RunEvidenceVerdict::Passed,
        }
    }

    pub(crate) fn summary(&self) -> String {
        format!(
            "{} ({} required, {} passed, {} failed, {} missing fact, {} timed out, {} unsupported; {} required blocked)",
            self.status,
            self.required,
            self.passed,
            self.failed,
            self.missing_fact,
            self.timed_out,
            self.unsupported,
            self.blocked_required
        )
    }

    pub(crate) fn report_details(&self) -> Vec<String> {
        let mut details = vec![format!("manifest assertions: {}", self.summary())];
        for result in &self.results {
            let mut line = format!(
                "assertion {} `{}` required={}: {}",
                result.status, result.id, result.required, result.description
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
pub(crate) const ASSERTION_STATUS_MISSING_FACT: &str = "missing_fact";
pub(crate) const ASSERTION_STATUS_TIMED_OUT: &str = "timed_out";
pub(crate) const ASSERTION_STATUS_UNSUPPORTED: &str = "unsupported";
pub(crate) const ASSERTION_STATUS_INCONCLUSIVE: &str = "inconclusive";

fn required_by_default() -> bool {
    true
}

impl RunEvidenceArtifact {
    pub(crate) fn record_assertions(&mut self, assertions: AssertionEvidence) {
        let assertion_verdict = assertions.verdict();
        self.assertions = Some(assertions);
        self.verdict = match (self.verdict, assertion_verdict) {
            (RunEvidenceVerdict::Failed, _) | (_, RunEvidenceVerdict::Failed) => {
                RunEvidenceVerdict::Failed
            }
            (RunEvidenceVerdict::Inconclusive, _) | (_, RunEvidenceVerdict::Inconclusive) => {
                RunEvidenceVerdict::Inconclusive
            }
            _ => RunEvidenceVerdict::Passed,
        };
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn result(id: &str, status: &str, required: bool) -> AssertionResultEvidence {
        AssertionResultEvidence {
            id: id.to_string(),
            required,
            status: status.to_string(),
            description: id.to_string(),
            artifact: None,
            kind: None,
            phase: None,
            command: None,
            cwd: None,
            context_path: None,
            stdout_path: None,
            stderr_path: None,
            status_path: None,
            exit_status: None,
            timeout_ms: None,
            duration_ms: None,
            details: Vec::new(),
        }
    }

    #[test]
    fn unsupported_optional_assertion_remains_visible_without_blocking() {
        let assertions = AssertionEvidence::from_results(vec![
            result("required", ASSERTION_STATUS_PASSED, true),
            result("optional", ASSERTION_STATUS_UNSUPPORTED, false),
        ]);

        assert_eq!(assertions.status, ASSERTION_STATUS_PASSED);
        assert_eq!(assertions.unsupported, 1);
        assert_eq!(assertions.blocked_required, 0);
        assert!(!assertions.has_failures());
    }

    #[test]
    fn every_nonpassing_required_outcome_blocks_success() {
        for (status, verdict) in [
            (ASSERTION_STATUS_FAILED, RunEvidenceVerdict::Failed),
            (ASSERTION_STATUS_TIMED_OUT, RunEvidenceVerdict::Failed),
            (
                ASSERTION_STATUS_MISSING_FACT,
                RunEvidenceVerdict::Inconclusive,
            ),
            (
                ASSERTION_STATUS_UNSUPPORTED,
                RunEvidenceVerdict::Inconclusive,
            ),
        ] {
            let assertions = AssertionEvidence::from_results(vec![result("proof", status, true)]);
            assert!(assertions.has_failures(), "{status}");
            assert_eq!(assertions.verdict(), verdict, "{status}");
        }
    }
}
