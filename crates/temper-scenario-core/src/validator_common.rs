// SPDX-License-Identifier: MPL-2.0

use std::fmt;

use serde::{Deserialize, Serialize};

/// Repository under validation, as selected by a workflow binding.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct TargetRepository {
    /// Forge repository in `owner/name` form.
    pub repo: String,
    /// Default branch whose merged state is being validated.
    pub default_branch: String,
    /// Optional Forge or browser URL for the repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl TargetRepository {
    /// Build a repository pointer from `owner/name` and default branch strings.
    pub fn new(repo: impl Into<String>, default_branch: impl Into<String>) -> Self {
        Self {
            repo: repo.into(),
            default_branch: default_branch.into(),
            url: None,
        }
    }

    /// Attach a human/browser URL to the repository pointer.
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }
}

/// Flexible reference to a forge or workflow artifact.
///
/// Workflow-defined validation targets are not limited to pull requests, so this
/// type deliberately allows issue numbers, PR numbers, opaque artifact ids, and
/// commit/branch pointers to be combined when a binding needs them.
#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArtifactReference {
    /// Workflow-local or forge-local artifact identifier when no numeric handle exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_id: Option<String>,
    /// Issue number for issue, parent-plan, or epic targets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_number: Option<u64>,
    /// Pull request number for implementation PR targets or related PRs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_number: Option<u64>,
    /// Branch name when the reference is branch-scoped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// General commit SHA pointer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
    /// Commit observed as the merged/default-branch SHA for a PR validation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged_main_sha: Option<String>,
    /// Commit observed on the default branch while preparing the bundle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_main_sha: Option<String>,
}

impl ArtifactReference {
    /// Reference an issue-like artifact by number.
    pub fn issue(number: u64) -> Self {
        Self {
            issue_number: Some(number),
            ..Self::default()
        }
    }

    /// Reference a pull request by number.
    pub fn pull_request(number: u64) -> Self {
        Self {
            pr_number: Some(number),
            ..Self::default()
        }
    }

    /// Reference an opaque workflow artifact id.
    pub fn artifact_id(id: impl Into<String>) -> Self {
        Self {
            artifact_id: Some(id.into()),
            ..Self::default()
        }
    }

    /// Attach the merged/default-branch commit SHA observed for this reference.
    pub fn with_merged_main_sha(mut self, sha: impl Into<String>) -> Self {
        self.merged_main_sha = Some(sha.into());
        self
    }

    /// Attach the current default-branch commit SHA observed for this reference.
    pub fn with_observed_main_sha(mut self, sha: impl Into<String>) -> Self {
        self.observed_main_sha = Some(sha.into());
        self
    }

    /// Attach a general commit SHA pointer to this reference.
    pub fn with_sha(mut self, sha: impl Into<String>) -> Self {
        self.sha = Some(sha.into());
        self
    }

    /// Attach a branch pointer to this reference.
    pub fn with_branch(mut self, branch: impl Into<String>) -> Self {
        self.branch = Some(branch.into());
        self
    }

    /// Stable, compact text used by summaries and Markdown renderers.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(number) = self.pr_number {
            parts.push(format!("PR #{number}"));
        }
        if let Some(number) = self.issue_number {
            parts.push(format!("issue #{number}"));
        }
        if let Some(id) = self.artifact_id.as_deref() {
            parts.push(id.to_string());
        }
        if let Some(branch) = self.branch.as_deref() {
            parts.push(format!("branch `{branch}`"));
        }
        if let Some(sha) = self.sha.as_deref() {
            parts.push(format!("sha `{sha}`"));
        }
        if let Some(sha) = self.merged_main_sha.as_deref() {
            parts.push(format!("merged `{sha}`"));
        }
        if let Some(sha) = self.observed_main_sha.as_deref() {
            parts.push(format!("observed `{sha}`"));
        }

        if parts.is_empty() {
            "unspecified".to_string()
        } else {
            parts.join(", ")
        }
    }
}

impl fmt::Display for ArtifactReference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.summary())
    }
}

/// A typed relationship from one artifact to another.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArtifactLink {
    /// Relationship name, such as `parent`, `depends_on`, or `produced_by`.
    pub relation: String,
    /// Artifact kind at the far side of the relationship.
    pub kind: String,
    /// Flexible reference to the related artifact.
    #[serde(rename = "ref")]
    pub reference: ArtifactReference,
    /// Repository for cross-repo relationships; omitted when it matches the target repo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
}

impl ArtifactLink {
    /// Build a relationship link to a referenced artifact.
    pub fn new(
        relation: impl Into<String>,
        kind: impl Into<String>,
        reference: ArtifactReference,
    ) -> Self {
        Self {
            relation: relation.into(),
            kind: kind.into(),
            reference,
            repo: None,
        }
    }

    /// Attach an explicit repository when the related artifact is cross-repo.
    pub fn with_repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = Some(repo.into());
        self
    }
}

/// Name/value fact captured from workflow evaluation.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkflowFact {
    /// Stable fact name.
    pub name: String,
    /// Human-readable, serialized fact value.
    pub value: String,
}

impl WorkflowFact {
    /// Build a workflow fact from a name and value.
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}
