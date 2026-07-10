//! Raw transition effects and portable gate conditions.
//!
//! Split from the spec root so the effect and condition vocabularies stay
//! separate from the structural declarations (roles, queues, transitions). These
//! are serde-loadable, untrusted vocabulary types; [`crate::validate`] resolves
//! them into the validated model.

use serde::{Deserialize, Serialize};
use temper_forge::ReviewDecision;

/// A raw transition effect.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RawEffect {
    /// Add a label to the target artifact. References a label id.
    AddLabel { label: String },
    /// Remove a label from the target artifact. References a label id.
    ///
    /// By default the label must be present when the transition plans. Set
    /// `if_present` for cleanup/handoff labels that should be cleared when
    /// present without making the transition stale when already absent.
    RemoveLabel {
        label: String,
        #[serde(default)]
        if_present: bool,
    },
    /// Assign the target artifact to the worker/user resolved for `role`.
    ///
    /// The payload references a declared workflow role, not a concrete Forge
    /// user. Runtime resolution of role-to-user/worker is deferred to the
    /// executor/runner layer.
    SetAssignee { role: String },
    /// Remove the assignee resolved for `role` from the target artifact.
    ///
    /// As with [`RawEffect::SetAssignee`], `role` is a declared workflow role
    /// id rather than a concrete Forge user id.
    RemoveAssignee { role: String },
    /// Post a prose/template comment body on the target artifact.
    CreateComment { body: String },
    /// Request creation of a pull request.
    ///
    /// `correlation_key`, when present, identifies retries of the same create
    /// request. Branches, title, body, and labels come from runtime context at
    /// execution time.
    ///
    /// `artifact_kind`, when present, names the pull-request artifact kind being
    /// created. Validation guarantees the kind exists and targets pull requests;
    /// runtimes can then derive creation labels and metadata from workflow
    /// declarations while preserving the older generic effect when it is omitted.
    CreatePullRequest {
        #[serde(default)]
        correlation_key: Option<String>,
        #[serde(default)]
        artifact_kind: Option<String>,
    },
    /// Request reviews from users resolved for workflow roles on the target PR.
    RequestReviewers { roles: Vec<String> },
    /// Submit a native pull-request review as the backend client's current user.
    SubmitReview { decision: ReviewDecision },
    /// Write an agent-authored body onto the target artifact.
    ///
    /// `correlation_key`, when present, identifies retries of the same authored
    /// write. The body text itself comes from the workspace work product through
    /// a runtime-input seam at execution time, not from this declaration.
    SetBody {
        #[serde(default)]
        correlation_key: Option<String>,
    },
    /// Submit a native pull-request review carrying a runtime-supplied body.
    ///
    /// `decision` is the portable review verdict this transition submits;
    /// `correlation_key`, when present, identifies retries. The review body
    /// comes from the workspace work product through a runtime-input seam at
    /// execution time.
    AttachReview {
        decision: ReviewDecision,
        #[serde(default)]
        correlation_key: Option<String>,
    },
    /// Create one-or-many child issues from the workspace work product.
    ///
    /// The children — their authored titles, bodies, labels, and the
    /// parent/dependency relations between them — come from the workspace work
    /// product through a runtime-input seam at execution time, not from this
    /// declaration. This is the principled, in-workflow form of architect
    /// fan-out: one verdict drives a plan of dependent children.
    ///
    /// `correlation_key`, when present, is the base key under which the children
    /// are made idempotent; each child derives a stable per-child key from it so
    /// a retry reuses the existing children instead of duplicating them.
    ///
    /// `record_parent_dependencies`, when true, records every created child as a
    /// dependency of the source issue after all children exist and sibling
    /// dependency slugs have been linked. The default `false` preserves legacy
    /// same-repository fan-out behavior, while cross-repository fan-outs continue
    /// to record parent dependencies for compatibility.
    CreateIssues {
        #[serde(default)]
        correlation_key: Option<String>,
        #[serde(default)]
        record_parent_dependencies: bool,
        /// Minimum authored child count. `create_issues` always requires at
        /// least one product; the default preserves existing workflows.
        #[serde(default = "default_min_children")]
        min_children: usize,
        /// Optional upper bound for workflows that require a fixed-size fan-out.
        #[serde(default)]
        max_children: Option<usize>,
    },
    /// Request merging the target pull request. Carries no portable payload.
    MergePullRequest,
    /// Close the parent issue(s) recorded in the pull-request artifact's workflow
    /// metadata. Reads `WorkflowMetadata::parents` from the PR body and, for each
    /// same-repository parent, closes the issue (state → Closed) and clears the
    /// `in-progress` label if present. Already-closed parents are idempotent
    /// no-ops; absent/missing parent metadata is not an error.
    CloseParentIssues,
}

fn default_min_children() -> usize {
    1
}

/// A portable condition that can satisfy a gate without a workflow transition.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RawGateCondition {
    /// The artifact must carry this label.
    LabelPresent { label: String },
    /// The artifact must occupy `state` within `dimension`.
    StateEquals { dimension: String, state: String },
    /// Every `dependency` relation target of the artifact must have landed
    /// (its prerequisite work merged). Which targets have landed is supplied by
    /// the runtime as an external signal; the condition references relations by
    /// kind, so it carries no payload.
    DependenciesResolved,
    /// The artifact's native CI must have passed. Whether CI passed is supplied
    /// by the runtime as a signal computed from the Forge's `CiJob`
    /// conclusions (see ADR 0014); the condition references the artifact's CI,
    /// so it carries no payload.
    CiPassed,
    /// The artifact's native CI must have completed with a non-success result.
    /// The runtime computes this from the same Forge `CiJob` data as
    /// [`RawGateCondition::CiPassed`].
    CiFailed,
    /// The pull request's native review aggregate must be approved.
    ReviewApproved,
    /// Some reviewer's latest native review decision must request changes.
    ReviewChangesRequested,
}
