// SPDX-License-Identifier: MPL-2.0

use serde::{Deserialize, Serialize};

use crate::ArtifactLink;

/// Comment fact with provenance back to the artifact that contained it.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct CommentContext {
    /// Artifact where the comment appeared.
    pub artifact: ArtifactLink,
    /// Comment author.
    pub author: String,
    /// Timestamp captured as an RFC3339-like string.
    pub timestamp: String,
    /// Bounded comment body.
    pub body: String,
    /// Human/browser URL for the comment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Review fact for a pull request in scope.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReviewContext {
    /// Pull request under review.
    pub pr_number: u64,
    /// Reviewer who submitted a decision, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer: Option<String>,
    /// Requested reviewers still relevant to the PR.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requested_reviewers: Vec<String>,
    /// Review decision such as `approved`, `changes_requested`, or `commented`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
    /// Bounded review body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Discussion thread summaries.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub threads: Vec<ReviewThreadContext>,
    /// Human/browser URL for the review.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Review discussion thread summary.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReviewThreadContext {
    /// Provider thread id.
    pub id: String,
    /// Thread state, such as `resolved` or `open`.
    pub state: String,
    /// Concise thread summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Human/browser URL for the thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Diff pointer and concise file summary for a PR or aggregate range.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DiffContext {
    /// Artifact or range this diff describes.
    pub scope: ArtifactLink,
    /// Changed files in deterministic order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed_files: Vec<ChangedFileContext>,
    /// Diffstat text, if available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diffstat: Option<String>,
    /// Paths the bundle generator considered notable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notable_paths: Vec<String>,
    /// Pointer to a raw diff artifact rather than embedded unbounded diff text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_diff_uri: Option<String>,
    /// Concise diff summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// One changed file entry in a bounded diff summary.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ChangedFileContext {
    /// File path.
    pub path: String,
    /// Provider status, such as `added`, `modified`, or `removed`.
    pub status: String,
    /// Added line count when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additions: Option<u64>,
    /// Deleted line count when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deletions: Option<u64>,
}

/// CI job or workflow run pointer.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct CiRunContext {
    /// Artifact or aggregate range the CI run covers.
    pub scope: ArtifactLink,
    /// CI job or workflow name.
    pub job_name: String,
    /// CI conclusion such as `success`, `failure`, or `cancelled`.
    pub conclusion: String,
    /// Head SHA for the CI run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_sha: Option<String>,
    /// Human/browser URL for the run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_url: Option<String>,
    /// Log pointer rather than embedded unbounded log text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_uri: Option<String>,
    /// Artifact pointers emitted by the run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_uris: Vec<String>,
}

/// Metadata for a checked-in scenario related to the validation target.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScenarioMetadataContext {
    /// Scenario name.
    pub name: String,
    /// Repo-relative scenario path.
    pub path: String,
    /// Scenario status from the manifest.
    pub status: String,
    /// Scenario stability from the manifest.
    pub stability: String,
    /// Assertion templates advertised by the scenario.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub templates: Vec<String>,
    /// Commit containing the scenario metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// Deterministic feature-to-scenario mapping identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mapping_id: Option<String>,
    /// Mapped feature issue in `owner/name#number` form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feature: Option<String>,
    /// Mapped plan issue in `owner/name#number` form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// Feature source branch declared by the scenario mapping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_branch: Option<String>,
    /// Deterministic digest of the resolved manifest and scenario content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

/// Suggested scenario or ad-hoc validation case for the validator to inspect.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SuggestedScenario {
    /// Scenario name or proposed case name.
    pub name: String,
    /// Why this case is relevant to the target.
    pub rationale: String,
    /// Expected observations or signals.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expected_signals: Vec<String>,
}

/// Workflow facts attached to a validator handoff.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkflowContext {
    /// Validator role id.
    pub role_id: String,
    /// Validator action id.
    pub action_id: String,
    /// Queue id that received the validator work item.
    pub queue_id: String,
    /// Workflow artifact ids correlated with this handoff.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_ids: Vec<String>,
    /// Workflow labels visible at handoff time.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    /// Workflow relationships relevant to validation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relationships: Vec<ArtifactLink>,
    /// Workflow gates that constrain validation readiness.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gates: Vec<WorkflowGate>,
    /// Trigger reason repeated from the workflow layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_reason: Option<String>,
    /// Workflow-sourced acceptance criteria.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acceptance_criteria: Vec<String>,
}

impl WorkflowContext {
    /// Build workflow context from role/action/queue ids.
    pub fn new(
        role_id: impl Into<String>,
        action_id: impl Into<String>,
        queue_id: impl Into<String>,
    ) -> Self {
        Self {
            role_id: role_id.into(),
            action_id: action_id.into(),
            queue_id: queue_id.into(),
            artifact_ids: Vec::new(),
            labels: Vec::new(),
            relationships: Vec::new(),
            gates: Vec::new(),
            trigger_reason: None,
            acceptance_criteria: Vec::new(),
        }
    }
}

/// Workflow gate state captured in a context bundle.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkflowGate {
    /// Gate id.
    pub id: String,
    /// Gate state, such as `passed`, `blocked`, or `pending`.
    pub state: String,
    /// Optional human-readable gate summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}
