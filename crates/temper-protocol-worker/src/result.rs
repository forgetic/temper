// SPDX-License-Identifier: MPL-2.0

use serde::{Deserialize, Serialize};
use serde_json::Value;
use temper_protocol_activity::ModelFailureV1;
use temper_verdict::{VerdictChildView, VerdictResultView};

use crate::SessionRecoveryEvidenceV1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultStatus {
    Success,
    Failure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    Transient,
    Permanent,
    Canceled,
    Protocol,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Branch {
    pub name: String,
    pub head_sha: String,
}

/// The pushed product of one writable repository in a coordinated head result.
/// The daemon opens one pull request per outcome.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepoOutcome {
    /// Repository path, `owner/name`.
    pub repo: String,
    pub branch: Branch,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Failure {
    pub class: FailureClass,
    pub message: String,
    /// Canonical model diagnostic, independent of optional activity tracing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_failure: Option<ModelFailureV1>,
    /// Durable decision/evidence from the bounded session recovery policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_recovery: Option<SessionRecoveryEvidenceV1>,
}

impl Failure {
    /// Canonicalizes typed evidence at a worker/daemon trust boundary. Invalid
    /// recovery evidence is discarded rather than allowing text or a mismatched
    /// attempt identity into typed fields.
    pub fn normalize_evidence(&mut self, expected_attempt_id: Option<&str>) {
        if let Some(model_failure) = &mut self.model_failure {
            model_failure.normalize();
        }
        if self
            .session_recovery
            .as_ref()
            .is_some_and(|evidence| evidence.validate_for_attempt(expected_attempt_id).is_err())
        {
            self.session_recovery = None;
        }
    }
}

/// One workspace-authored child issue carried by a breakdown verdict result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobChild {
    /// Stable per-child identifier within the result (seeds the child's
    /// correlation key; referenced by sibling `depends_on`).
    pub slug: String,
    pub title: String,
    pub body: String,
    /// Workflow artifact kind for this child issue. Omitted defaults to `code`
    /// when the daemon applies verdict child fan-out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    /// Slugs of sibling children in the same result that must land before
    /// this one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    /// Target repository as an `owner/name` path (the same shape the daemon's
    /// `--repo` flag parses). `None` = the job's own repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_repo: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobResult {
    pub protocol_version: u32,
    pub worker_id: String,
    pub job_id: String,
    /// Opaque assignment fence copied byte-for-byte from [`crate::Assign`].
    /// Optional only for compatibility deserialization; daemons reject an
    /// unfenced result for a fenced assignment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<String>,
    pub status: ResultStatus,
    /// Per-repo head products -- one per writable repo that produced a diff. The
    /// daemon opens one pull request per entry. Empty for a verdict result.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repos: Vec<RepoOutcome>,
    /// Verdict chosen by a verdict job (must be one of the assignment's
    /// `allowed_verdicts`). A success result may carry a verdict and no repos.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<String>,
    /// Agent-authored PR title. Without a verdict this is the implementation PR
    /// handoff title. With a verdict it is used only by routed transitions whose
    /// `create_pull_request` effect declares a PR artifact kind; other verdict
    /// effects ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Authored body. With a verdict, this is the verdict payload (e.g. the
    /// rewritten issue spec or the review body). Without a verdict, this is the
    /// implementation PR report body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Child issues authored by a breakdown verdict (e.g. `needs_breakdown`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<JobChild>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<Failure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl JobResult {
    /// Normalizes optional failure evidence after deserialization and before
    /// durable admission.
    pub fn normalize_failure_evidence(&mut self) {
        let attempt_id = self.attempt_id.clone();
        if let Some(failure) = &mut self.failure {
            failure.normalize_evidence(attempt_id.as_deref());
        }
    }
}

impl VerdictResultView for JobResult {
    type Child = JobChild;

    fn verdict(&self) -> Option<&str> {
        self.verdict.as_deref()
    }

    fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    fn body(&self) -> Option<&str> {
        self.body.as_deref()
    }

    fn children(&self) -> &[Self::Child] {
        &self.children
    }
}

impl VerdictChildView for JobChild {
    fn slug(&self) -> &str {
        &self.slug
    }

    fn title(&self) -> &str {
        &self.title
    }

    fn body(&self) -> &str {
        &self.body
    }

    fn kind(&self) -> Option<&str> {
        self.kind.as_deref()
    }

    fn depends_on(&self) -> &[String] {
        &self.depends_on
    }
}
