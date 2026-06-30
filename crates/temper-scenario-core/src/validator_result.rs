// SPDX-License-Identifier: MPL-2.0

use serde::{Deserialize, Serialize};

use crate::{
    AcceptanceCriterion, ArtifactReference, EvidenceEntry, EvidenceKind, FollowUpIssueIntent,
    ValidatedClaim, ValidationReport, ValidationStatus, ValidationVerdict,
};

/// Stable schema id for workflow-native validator results.
pub const VALIDATOR_RESULT_SCHEMA: &str = "temper.validator.result.v1";

/// Structured validator output accepted by future workflow-native validation.
///
/// The target is generalized beyond pull requests while related PR entries keep
/// PR-level merge and source-issue facts available for aggregate validations.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ValidatorResult {
    /// Stable schema id, currently [`VALIDATOR_RESULT_SCHEMA`].
    pub schema: String,
    /// Selected artifact that was actually validated.
    pub target: ValidatorResultTarget,
    /// Pull requests related to the target. Aggregate targets can have many.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related_prs: Vec<RelatedPullRequest>,
    /// Overall validator verdict.
    pub verdict: ValidationVerdict,
    /// Claims the validator attempted to prove or observe.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub validated_claims: Vec<ValidationAssertion>,
    /// Observable acceptance criteria checked by the validator.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acceptance_criteria: Vec<ValidationAssertion>,
    /// Evidence entries cited by claims and criteria.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<StructuredEvidenceEntry>,
    /// Missing proof, omitted context, flaky systems, or other limitations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub limitations: Vec<String>,
    /// Optional workflow-owned follow-up issue creation intent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_up_issue: Option<FollowUpIssueIntent>,
    /// Optional intent to promote an ad-hoc validation case into a scenario.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scenario_promotion: Option<ScenarioPromotionIntent>,
}

impl ValidatorResult {
    /// Build an empty structured result for a selected target.
    pub fn new(target: ValidatorResultTarget, verdict: ValidationVerdict) -> Self {
        Self {
            schema: VALIDATOR_RESULT_SCHEMA.to_string(),
            target,
            related_prs: Vec::new(),
            verdict,
            validated_claims: Vec::new(),
            acceptance_criteria: Vec::new(),
            evidence: Vec::new(),
            limitations: Vec::new(),
            follow_up_issue: None,
            scenario_promotion: None,
        }
    }

    /// Convert the temporary PR-only Markdown report model into the structured
    /// schema without losing fields currently captured by `validate-pr`.
    pub fn from_validation_report(report: ValidationReport, repo: impl Into<String>) -> Self {
        let ValidationReport {
            target,
            verdict,
            validated_claims,
            acceptance_criteria,
            evidence,
            limitations,
            follow_up,
        } = report;

        let merged_main_sha = target.merged_main_sha;
        let target = ValidatorResultTarget {
            kind: "implementation_pr".to_string(),
            repo: repo.into(),
            reference: ArtifactReference::pull_request(target.pr_number)
                .with_merged_main_sha(merged_main_sha.clone()),
            trigger_reason: Some("temporary validate-pr bridge".to_string()),
            state_fingerprint: Some(format!("pr:{}:main:{merged_main_sha}", target.pr_number)),
            title: None,
            labels: Vec::new(),
        };

        let related_prs = vec![RelatedPullRequest {
            pr_number: target.reference.pr_number.unwrap_or_default(),
            source_issue: None,
            merged_main_sha: Some(merged_main_sha),
            observed_main_sha: None,
            title: None,
            url: None,
        }];

        Self {
            schema: VALIDATOR_RESULT_SCHEMA.to_string(),
            target,
            related_prs,
            verdict,
            validated_claims: validated_claims
                .into_iter()
                .map(ValidationAssertion::from_claim)
                .collect(),
            acceptance_criteria: acceptance_criteria
                .into_iter()
                .map(ValidationAssertion::from_criterion)
                .collect(),
            evidence: evidence
                .into_iter()
                .enumerate()
                .map(|(index, entry)| StructuredEvidenceEntry::from_report_entry(index, entry))
                .collect(),
            limitations,
            follow_up_issue: follow_up,
            scenario_promotion: None,
        }
    }
}

/// Generalized selected target for a structured validator result.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ValidatorResultTarget {
    /// Workflow-defined kind, such as `implementation_pr`, `issue`, or `epic`.
    pub kind: String,
    /// Repository in `owner/name` form.
    pub repo: String,
    /// Concrete target reference. Serialized as `ref` to match the handoff schema.
    #[serde(rename = "ref")]
    pub reference: ArtifactReference,
    /// Why this target was validated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_reason: Option<String>,
    /// Idempotency or aggregate-state fingerprint for the validated state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_fingerprint: Option<String>,
    /// Optional target title for rendered reports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Target labels at validation time.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
}

impl ValidatorResultTarget {
    /// Build a generalized validator result target.
    pub fn new(
        kind: impl Into<String>,
        repo: impl Into<String>,
        reference: ArtifactReference,
    ) -> Self {
        Self {
            kind: kind.into(),
            repo: repo.into(),
            reference,
            trigger_reason: None,
            state_fingerprint: None,
            title: None,
            labels: Vec::new(),
        }
    }
}

/// Pull request facts related to a structured validator result.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct RelatedPullRequest {
    /// Pull request number.
    pub pr_number: u64,
    /// Source issue number when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_issue: Option<u64>,
    /// SHA that landed on the default branch for this PR.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged_main_sha: Option<String>,
    /// Default-branch SHA observed during validation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_main_sha: Option<String>,
    /// Optional PR title for rendered reports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Optional human/browser URL for the PR.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Claim or acceptance criterion status plus evidence references.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ValidationAssertion {
    /// Claim or criterion text.
    pub description: String,
    /// Status using the #55 validation report vocabulary.
    pub status: ValidationStatus,
    /// Evidence entry ids cited by this assertion.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_refs: Vec<String>,
}

impl ValidationAssertion {
    /// Build an assertion from text and status.
    pub fn new(description: impl Into<String>, status: ValidationStatus) -> Self {
        Self {
            description: description.into(),
            status,
            evidence_refs: Vec::new(),
        }
    }

    /// Cite an evidence entry id.
    pub fn with_evidence_ref(mut self, evidence_ref: impl Into<String>) -> Self {
        self.evidence_refs.push(evidence_ref.into());
        self
    }

    fn from_claim(claim: ValidatedClaim) -> Self {
        Self {
            description: claim.description,
            status: claim.status,
            evidence_refs: claim.evidence,
        }
    }

    fn from_criterion(criterion: AcceptanceCriterion) -> Self {
        Self {
            description: criterion.description,
            status: criterion.status,
            evidence_refs: criterion.evidence,
        }
    }
}

/// Structured evidence entry cited by validation assertions.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct StructuredEvidenceEntry {
    /// Stable evidence id unique within the result.
    pub id: String,
    /// Evidence kind using the #55 validation report vocabulary.
    pub kind: EvidenceKind,
    /// One-line evidence summary.
    pub summary: String,
    /// Optional bounded details.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    /// Optional external URI pointer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    /// Optional local artifact path pointer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_path: Option<String>,
}

impl StructuredEvidenceEntry {
    /// Build an evidence entry from id, kind, and summary.
    pub fn new(id: impl Into<String>, kind: EvidenceKind, summary: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind,
            summary: summary.into(),
            details: None,
            uri: None,
            artifact_path: None,
        }
    }

    fn from_report_entry(index: usize, entry: EvidenceEntry) -> Self {
        Self {
            id: format!("evidence-{}", index + 1),
            kind: entry.kind,
            summary: entry.summary,
            details: (!entry.details.is_empty()).then(|| entry.details.join("\n")),
            uri: None,
            artifact_path: None,
        }
    }
}

/// Intent to turn validation knowledge into a checked-in scenario.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScenarioPromotionIntent {
    /// Proposed scenario name or slug.
    pub scenario_name: String,
    /// Scenario intent statement.
    pub intent: String,
    /// Evidence entry ids or artifact pointers that motivate promotion.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_evidence: Vec<String>,
    /// Notes about fixtures needed by the promoted scenario.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixture_notes: Option<String>,
    /// Whether the validator proposes opening an issue for scenario promotion.
    #[serde(default)]
    pub propose_issue: bool,
    /// Whether the validator proposes opening a PR for scenario promotion.
    #[serde(default)]
    pub propose_pr: bool,
}
