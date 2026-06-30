// SPDX-License-Identifier: MPL-2.0

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::validator_context_sections::{
    CiRunContext, CommentContext, DiffContext, ReviewContext, ScenarioMetadataContext,
    SuggestedScenario, WorkflowContext,
};
use crate::{ArtifactLink, ArtifactReference, TargetRepository, WorkflowFact};

/// Stable schema id for workflow-native validator context bundles.
pub const VALIDATOR_CONTEXT_SCHEMA: &str = "temper.validator.context.v1";

/// Prepared, read-only context bundle handed to a validator role.
///
/// The bundle records the workflow-selected target plus bounded Forge,
/// repository, scenario, CI, diff, and workflow facts. It is intentionally a
/// data model only: generation from live Forgejo state is a later workflow
/// feature.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ValidatorContext {
    /// Stable schema id, currently [`VALIDATOR_CONTEXT_SCHEMA`].
    pub schema: String,
    /// Repository and default branch under validation.
    pub target_repo: TargetRepository,
    /// Workflow-selected validation target.
    pub target: ValidatorTarget,
    /// Binding that selected the target and made the work idempotent.
    pub validation_binding: ValidationBindingSummary,
    /// Pull requests in scope. Per-PR validation normally has one entry.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pull_requests: Vec<PullRequestContext>,
    /// Issues in scope, including selected target issues and related children.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<IssueContext>,
    /// Aggregate rollup when the selected target is a parent issue, plan, epic,
    /// or other workflow-defined aggregate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggregate: Option<AggregateContext>,
    /// Validation-relevant issue or PR comments.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<CommentContext>,
    /// Review decisions, requests, bodies, and thread summaries for PRs in scope.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reviews: Vec<ReviewContext>,
    /// Diff pointers and changed-file summaries for each PR or aggregate range.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diffs: Vec<DiffContext>,
    /// CI jobs, run URLs, and log/artifact pointers relevant to the target.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ci: Vec<CiRunContext>,
    /// Checked-in scenarios related to the target or aggregate.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scenario_metadata: Vec<ScenarioMetadataContext>,
    /// Suggested scenarios or ad-hoc validation cases with rationale.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggested_scenarios: Vec<SuggestedScenario>,
    /// Workflow-level facts that produced and constrain the handoff.
    pub workflow: WorkflowContext,
}

impl ValidatorContext {
    /// Build an empty context bundle for a selected target and binding.
    pub fn new(
        target_repo: TargetRepository,
        target: ValidatorTarget,
        validation_binding: ValidationBindingSummary,
        workflow: WorkflowContext,
    ) -> Self {
        Self {
            schema: VALIDATOR_CONTEXT_SCHEMA.to_string(),
            target_repo,
            target,
            validation_binding,
            pull_requests: Vec::new(),
            issues: Vec::new(),
            aggregate: None,
            comments: Vec::new(),
            reviews: Vec::new(),
            diffs: Vec::new(),
            ci: Vec::new(),
            scenario_metadata: Vec::new(),
            suggested_scenarios: Vec::new(),
            workflow,
        }
    }

    /// Deterministic, compact summary for logs, tests, and handoff previews.
    pub fn summary(&self) -> String {
        let mut output = String::new();
        let _ = writeln!(output, "schema: {}", self.schema);
        let _ = writeln!(
            output,
            "target: {} {} in {}",
            self.target.kind, self.target.reference, self.target_repo.repo
        );
        let _ = writeln!(
            output,
            "binding: {} ({} / {})",
            self.validation_binding.id,
            self.validation_binding.role_id,
            self.validation_binding.action_id
        );
        let pull_requests = self
            .pull_requests
            .iter()
            .map(pull_request_summary)
            .collect::<Vec<_>>();
        let _ = writeln!(output, "pull_requests: {}", list_or_none(&pull_requests));
        let issues = self.issues.iter().map(issue_summary).collect::<Vec<_>>();
        let _ = writeln!(output, "issues: {}", list_or_none(&issues));
        match &self.aggregate {
            Some(aggregate) => {
                let readiness = aggregate
                    .ready_rule
                    .as_deref()
                    .unwrap_or("unspecified readiness");
                let _ = writeln!(
                    output,
                    "aggregate: {} via {}",
                    aggregate.completion_rollup, readiness
                );
            }
            None => {
                let _ = writeln!(output, "aggregate: none");
            }
        }
        let _ = writeln!(
            output,
            "workflow: {} / {} / {}",
            self.workflow.role_id, self.workflow.action_id, self.workflow.queue_id
        );
        output
    }
}

/// The artifact selected for validation by a workflow binding.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ValidatorTarget {
    /// Workflow-defined target kind, such as `implementation_pr`, `issue`, or `epic`.
    pub kind: String,
    /// Concrete target reference. Serialized as `ref` to match the handoff schema.
    #[serde(rename = "ref")]
    pub reference: ArtifactReference,
    /// Target title when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Bounded body summary; large bodies should be represented by pointers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_summary: Option<String>,
    /// Human/browser URL for the selected target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Forge or workflow state at bundle creation time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// Target labels visible to the workflow binding.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    /// Why this target was selected for validation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_reason: Option<String>,
    /// Readiness facts that made the target eligible.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub readiness_facts: Vec<WorkflowFact>,
    /// Idempotency or aggregate-state fingerprint for the target state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_fingerprint: Option<String>,
}

impl ValidatorTarget {
    /// Build a selected target from a workflow kind and reference.
    pub fn new(kind: impl Into<String>, reference: ArtifactReference) -> Self {
        Self {
            kind: kind.into(),
            reference,
            title: None,
            body_summary: None,
            url: None,
            state: None,
            labels: Vec::new(),
            trigger_reason: None,
            readiness_facts: Vec::new(),
            state_fingerprint: None,
        }
    }
}

/// Summary of the workflow validation binding that produced a handoff.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ValidationBindingSummary {
    /// Stable workflow-local binding id.
    pub id: String,
    /// Validator role id assigned by the binding.
    pub role_id: String,
    /// Validator action/transition id assigned by the binding.
    pub action_id: String,
    /// Queue id for the validator work item.
    pub queue_id: String,
    /// Declared artifact kind the binding validates.
    pub target_artifact: String,
    /// Human-readable trigger summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_summary: Option<String>,
    /// Human-readable readiness summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readiness_summary: Option<String>,
    /// Aggregation rules named by the binding.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aggregation_rules: Vec<String>,
    /// Durable idempotency key for this target state.
    pub idempotency_key: String,
}

impl ValidationBindingSummary {
    /// Build a binding summary from the required workflow identifiers.
    pub fn new(
        id: impl Into<String>,
        role_id: impl Into<String>,
        action_id: impl Into<String>,
        queue_id: impl Into<String>,
        target_artifact: impl Into<String>,
        idempotency_key: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            role_id: role_id.into(),
            action_id: action_id.into(),
            queue_id: queue_id.into(),
            target_artifact: target_artifact.into(),
            trigger_summary: None,
            readiness_summary: None,
            aggregation_rules: Vec::new(),
            idempotency_key: idempotency_key.into(),
        }
    }
}

/// Pull request facts preserved in a validator context bundle.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct PullRequestContext {
    /// Pull request number.
    pub pr_number: u64,
    /// Pull request title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Bounded body summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_summary: Option<String>,
    /// Author login or display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Human/browser URL for the PR.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Forge state, such as `open`, `merged`, or `closed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// Merge timestamp captured as a string to keep the schema transport-neutral.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged_at: Option<String>,
    /// SHA that landed on the default branch for this PR.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged_main_sha: Option<String>,
    /// Default-branch SHA observed while preparing the context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_main_sha: Option<String>,
    /// Source issue number when the workflow can infer one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_issue: Option<u64>,
    /// Produced-PR relationship label, such as `implements`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub produced_pr_relation: Option<String>,
    /// PR labels relevant to validation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
}

/// Issue or epic facts preserved in a validator context bundle.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct IssueContext {
    /// Issue number.
    pub issue_number: u64,
    /// Workflow or Forge kind for this issue, such as `issue` or `epic`.
    pub kind: String,
    /// Issue title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Bounded body summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_summary: Option<String>,
    /// Human/browser URL for the issue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Forge or workflow state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// Issue labels relevant to validation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    /// Parent/child/dependency/design links involving this issue.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<ArtifactLink>,
    /// Child issues used for aggregate validation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub child_issues: Vec<u64>,
    /// Dependency issue numbers used for readiness.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<u64>,
    /// Produced PR numbers associated with this issue.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub produced_prs: Vec<u64>,
    /// Closing keywords or references observed in PRs/comments.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub closing_keywords: Vec<String>,
    /// Acceptance criteria extracted from the issue/workflow context.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acceptance_criteria: Vec<String>,
    /// Human-readable child completion state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_completion: Option<String>,
}

/// Aggregate validation rollup for parent issues, master plans, epics, or other
/// workflow-defined aggregate targets.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct AggregateContext {
    /// Target kind for the aggregate, such as `epic`.
    pub target_kind: String,
    /// Child issue inventory.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub child_issues: Vec<u64>,
    /// Pull requests included in the aggregate.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pull_requests: Vec<u64>,
    /// Human-readable completion rollup.
    pub completion_rollup: String,
    /// Remaining blockers at handoff time.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remaining_blockers: Vec<String>,
    /// Merged commits or range endpoints that make up the aggregate.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub merged_shas: Vec<String>,
    /// Rule or binding condition that made the aggregate ready.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready_rule: Option<String>,
    /// Aggregate-state fingerprint used for idempotency.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
}

fn pull_request_summary(pull_request: &PullRequestContext) -> String {
    let sha = pull_request
        .merged_main_sha
        .as_deref()
        .or(pull_request.observed_main_sha.as_deref())
        .unwrap_or("unknown");
    format!("#{}@{}", pull_request.pr_number, sha)
}

fn issue_summary(issue: &IssueContext) -> String {
    format!("#{} ({})", issue.issue_number, issue.kind)
}

fn list_or_none(values: &[String]) -> String {
    if values.is_empty() {
        "none".to_string()
    } else {
        values.join(", ")
    }
}
