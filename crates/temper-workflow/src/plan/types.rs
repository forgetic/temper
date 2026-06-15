//! Typed outputs a planner produces: effects, postconditions, and the plan.
//!
//! These are pure data types shared by the [`Planner`](super::Planner), the
//! [`Executor`](crate::execute), and the [`Reconciler`](crate::reconcile). They
//! live in their own module so `plan.rs` keeps the planning logic focused.

use crate::classify::ArtifactSource;
use crate::ids::{ArtifactKindId, LabelId, RoleId, TransitionId};
use crate::metadata::Lease;
use temper_forge_model::ReviewDecision;

/// A typed side effect a transition plan would apply.
///
/// This is a closed enum so executors and reconcilers must handle every
/// variant. Effects are relative to the plan's [`TransitionPlan::target`],
/// except the create variants, which request a brand-new artifact.
///
/// Transition specs can produce label, assignee, comment, pull-request create,
/// and merge effects. The executor applies those runtime effects except for the
/// lease placeholders, which are still rejected as unsupported before mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkflowEffect {
    /// Add a label to the target artifact. Produced from an `add_label` effect.
    AddLabel(LabelId),
    /// Remove a label from the target artifact. Produced from a `remove_label`
    /// effect.
    RemoveLabel(LabelId),
    /// Set the target artifact's assignee to the worker/user resolved for this
    /// declared workflow role.
    SetAssignee { role: RoleId },
    /// Remove the assignee resolved for this declared workflow role from the
    /// target artifact.
    RemoveAssignee { role: RoleId },
    /// Post a prose/template comment body on the target artifact.
    CreateComment { body: String },
    /// Request creation of a new issue, keyed for idempotent retries.
    CreateIssue { correlation_key: String },
    /// Request creation of a new pull request. The optional correlation key
    /// identifies retries; branch, title, body, and labels come from runtime
    /// context in a later execution phase.
    CreatePullRequest { correlation_key: Option<String> },
    /// Request reviews from users resolved for workflow roles on the target PR.
    RequestReviewers { roles: Vec<RoleId> },
    /// Submit a native pull-request review decision.
    SubmitReview { decision: ReviewDecision },
    /// Write an agent-authored body onto the target artifact. The optional
    /// correlation key identifies retries; the body text comes from runtime
    /// context (the workspace work product) in the execution phase, exactly as
    /// `CreatePullRequest` reads its head, branch, and title.
    SetBody { correlation_key: Option<String> },
    /// Submit a native pull-request review carrying a runtime-supplied body.
    /// The decision is portable workflow vocabulary; the body comes from runtime
    /// context (the workspace work product) in the execution phase. The optional
    /// correlation key identifies retries.
    AttachReview {
        decision: ReviewDecision,
        correlation_key: Option<String>,
    },
    /// Create one-or-many child artifacts from the workspace work product. The
    /// children (authored titles/bodies/labels and the parent/dependency
    /// relations between them) come from runtime context in the execution phase,
    /// exactly as `CreatePullRequest` reads its head. The optional correlation
    /// key is the base under which each child is made idempotent across retries.
    CreateIssues { correlation_key: Option<String> },
    /// Set or refresh the claim lease on the target artifact.
    ///
    /// Placeholder: leases are modeled in [`crate::metadata`] but no transition
    /// spec emits lease effects yet.
    UpdateLease { lease: Lease },
    /// Clear the claim lease on the target artifact.
    ///
    /// Placeholder (see [`WorkflowEffect::UpdateLease`]).
    ReleaseLease,
    /// Request merging the target pull request. Carries no portable payload.
    MergePullRequest,
}

/// A condition that must hold after a plan's effects are applied.
///
/// Postconditions let an executor verify a transition actually took effect on
/// fresh Forge state. They are derived from the transition's label and assignee
/// effects. Comment effects have no postcondition: a comment is an append-only
/// event, not a queryable state projection, so there is no after-the-fact
/// predicate to assert (the executor instead guarantees a comment is posted
/// at most once through an idempotency marker — see [`crate::execute`]).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Postcondition {
    /// The label must be present on the target artifact.
    LabelPresent(LabelId),
    /// The label must be absent from the target artifact.
    LabelAbsent(LabelId),
    /// The Forge user resolved for this role must be assigned to the target.
    AssigneePresent { role: RoleId },
    /// The Forge user resolved for this role must not be assigned to the target.
    AssigneeAbsent { role: RoleId },
}

/// A planned, not-yet-executed transition.
///
/// Produced by [`Planner::plan_transition`](super::Planner::plan_transition).
/// It records what would change (`effects`) and what must hold afterward
/// (`postconditions`) without applying anything. Planning is deterministic:
/// effects and postconditions follow the transition's declared effect order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransitionPlan {
    /// Transition that was planned.
    pub transition: TransitionId,
    /// Role the plan was authorized for.
    pub role: RoleId,
    /// Artifact kind the transition acts on.
    pub artifact: ArtifactKindId,
    /// Forge artifact the effects target.
    pub target: ArtifactSource,
    /// Typed effects to apply, in declaration order.
    pub effects: Vec<WorkflowEffect>,
    /// Conditions that must hold once the effects are applied.
    pub postconditions: Vec<Postcondition>,
}
