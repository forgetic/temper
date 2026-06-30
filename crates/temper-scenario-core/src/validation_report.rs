// SPDX-License-Identifier: MPL-2.0

use std::fmt;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

/// Pull request and merged/main commit under post-merge validation.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ValidationTarget {
    pub pr_number: u64,
    pub merged_main_sha: String,
}

impl ValidationTarget {
    pub fn new(pr_number: u64, merged_main_sha: impl Into<String>) -> Self {
        Self {
            pr_number,
            merged_main_sha: merged_main_sha.into(),
        }
    }
}

/// Overall validation result for a post-merge report.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationVerdict {
    Passed,
    Failed,
    Inconclusive,
}

impl ValidationVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Inconclusive => "inconclusive",
        }
    }
}

impl fmt::Display for ValidationVerdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Per-claim or per-criterion validation status.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub enum ValidationStatus {
    #[serde(rename = "satisfied")]
    Satisfied,
    #[serde(rename = "observed")]
    Observed,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "unproven")]
    Unproven,
    #[serde(rename = "not applicable")]
    NotApplicable,
}

impl ValidationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Satisfied => "satisfied",
            Self::Observed => "observed",
            Self::Failed => "failed",
            Self::Unproven => "unproven",
            Self::NotApplicable => "not applicable",
        }
    }
}

impl fmt::Display for ValidationStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A claim the validation attempted to prove or observe.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ValidatedClaim {
    pub description: String,
    pub status: ValidationStatus,
    pub evidence: Vec<String>,
}

impl ValidatedClaim {
    pub fn new(description: impl Into<String>, status: ValidationStatus) -> Self {
        Self {
            description: description.into(),
            status,
            evidence: Vec::new(),
        }
    }

    pub fn with_evidence(mut self, evidence: impl Into<String>) -> Self {
        self.evidence.push(evidence.into());
        self
    }
}

/// An observable acceptance criterion and the evidence associated with it.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct AcceptanceCriterion {
    pub description: String,
    pub status: ValidationStatus,
    pub evidence: Vec<String>,
}

impl AcceptanceCriterion {
    pub fn new(description: impl Into<String>, status: ValidationStatus) -> Self {
        Self {
            description: description.into(),
            status,
            evidence: Vec::new(),
        }
    }

    pub fn with_evidence(mut self, evidence: impl Into<String>) -> Self {
        self.evidence.push(evidence.into());
        self
    }
}

/// Durable categories for evidence captured by the temporary validator.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    ScenarioCheck,
    ScenarioRun,
    Command,
    Artifact,
    Observation,
}

impl EvidenceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ScenarioCheck => "scenario check",
            Self::ScenarioRun => "scenario run",
            Self::Command => "command",
            Self::Artifact => "artifact",
            Self::Observation => "observation",
        }
    }
}

impl fmt::Display for EvidenceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One observed validation fact, command result, or artifact pointer.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct EvidenceEntry {
    pub kind: EvidenceKind,
    pub summary: String,
    pub details: Vec<String>,
}

impl EvidenceEntry {
    pub fn new(kind: EvidenceKind, summary: impl Into<String>) -> Self {
        Self {
            kind,
            summary: summary.into(),
            details: Vec::new(),
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.details.push(detail.into());
        self
    }

    pub fn with_details<I, S>(mut self, details: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.details.extend(details.into_iter().map(Into::into));
        self
    }
}

/// Intent for a follow-up issue that should be opened after validation.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct FollowUpIssueIntent {
    pub title: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relation_hints: Vec<String>,
}

impl FollowUpIssueIntent {
    pub fn new(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            labels: Vec::new(),
            relation_hints: Vec::new(),
        }
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.labels.push(label.into());
        self
    }

    pub fn with_relation_hint(mut self, relation_hint: impl Into<String>) -> Self {
        self.relation_hints.push(relation_hint.into());
        self
    }
}

/// Durable post-merge validation report rendered as Markdown artifacts.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ValidationReport {
    pub target: ValidationTarget,
    pub verdict: ValidationVerdict,
    pub validated_claims: Vec<ValidatedClaim>,
    pub acceptance_criteria: Vec<AcceptanceCriterion>,
    pub evidence: Vec<EvidenceEntry>,
    pub limitations: Vec<String>,
    pub follow_up: Option<FollowUpIssueIntent>,
}

impl ValidationReport {
    pub fn new(
        pr_number: u64,
        merged_main_sha: impl Into<String>,
        verdict: ValidationVerdict,
    ) -> Self {
        Self {
            target: ValidationTarget::new(pr_number, merged_main_sha),
            verdict,
            validated_claims: Vec::new(),
            acceptance_criteria: Vec::new(),
            evidence: Vec::new(),
            limitations: Vec::new(),
            follow_up: None,
        }
    }

    pub fn render_markdown(&self) -> String {
        let mut output = String::new();
        let _ = writeln!(output, "# Post-merge validation report");
        let _ = writeln!(output);
        let _ = writeln!(output, "- PR: #{}", self.target.pr_number);
        let _ = writeln!(
            output,
            "- Merged/main SHA: `{}`",
            self.target.merged_main_sha
        );
        let _ = writeln!(output, "- Verdict: {}", self.verdict);
        let _ = writeln!(output);

        let _ = writeln!(output, "## Verdict");
        let _ = writeln!(output);
        let _ = writeln!(output, "Verdict: {}", self.verdict);
        let _ = writeln!(output);

        let _ = writeln!(output, "## Validated claims");
        let _ = writeln!(output);
        if self.validated_claims.is_empty() {
            let _ = writeln!(output, "- None recorded.");
        } else {
            for claim in &self.validated_claims {
                write_status_item(
                    &mut output,
                    &claim.description,
                    claim.status,
                    &claim.evidence,
                );
            }
        }
        let _ = writeln!(output);

        let _ = writeln!(output, "## Acceptance criteria");
        let _ = writeln!(output);
        if self.acceptance_criteria.is_empty() {
            let _ = writeln!(output, "- None recorded.");
        } else {
            for criterion in &self.acceptance_criteria {
                write_status_item(
                    &mut output,
                    &criterion.description,
                    criterion.status,
                    &criterion.evidence,
                );
            }
        }
        let _ = writeln!(output);

        let _ = writeln!(output, "## Evidence");
        let _ = writeln!(output);
        if self.evidence.is_empty() {
            let _ = writeln!(output, "- None recorded.");
        } else {
            for (index, entry) in self.evidence.iter().enumerate() {
                let _ = writeln!(
                    output,
                    "{}. **{}** — {}",
                    index + 1,
                    entry.kind,
                    entry.summary
                );
                for detail in &entry.details {
                    write_bullet(&mut output, detail, "   ");
                }
            }
        }
        let _ = writeln!(output);

        let _ = writeln!(output, "## Limitations");
        let _ = writeln!(output);
        if self.limitations.is_empty() {
            let _ = writeln!(output, "- None recorded.");
        } else {
            for limitation in &self.limitations {
                write_bullet(&mut output, limitation, "");
            }
        }
        let _ = writeln!(output);

        let _ = writeln!(output, "## Follow-up intent");
        let _ = writeln!(output);
        match &self.follow_up {
            Some(intent) => {
                let _ = writeln!(output, "- Title: {}", intent.title);
                if intent.labels.is_empty() {
                    let _ = writeln!(output, "- Labels: none");
                } else {
                    let _ = writeln!(output, "- Labels: {}", intent.labels.join(", "));
                }
                if !intent.relation_hints.is_empty() {
                    let _ = writeln!(
                        output,
                        "- Relation hints: {}",
                        intent.relation_hints.join(", ")
                    );
                }
                let _ = writeln!(output, "- Body:");
                write_blockquote(&mut output, &intent.body);
            }
            None => {
                let _ = writeln!(output, "- None recorded.");
            }
        }

        output
    }
}

fn write_status_item(
    output: &mut String,
    description: &str,
    status: ValidationStatus,
    evidence: &[String],
) {
    let _ = writeln!(output, "- [{}] {}", status, description);
    for item in evidence {
        write_bullet(output, item, "  ");
    }
}

fn write_bullet(output: &mut String, text: &str, indent: &str) {
    let mut lines = text.lines();
    let Some(first) = lines.next() else {
        let _ = writeln!(output, "{indent}-");
        return;
    };
    let _ = writeln!(output, "{indent}- {first}");
    for line in lines {
        let _ = writeln!(output, "{indent}  {line}");
    }
}

fn write_blockquote(output: &mut String, text: &str) {
    if text.is_empty() {
        let _ = writeln!(output, "> ");
        return;
    }
    for line in text.lines() {
        let _ = writeln!(output, "> {line}");
    }
}
