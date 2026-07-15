//! Runtime bindings the pure planner cannot know (Phase 9b/10).
//!
//! A workflow spec references *roles*, never concrete Forge users: an effect is
//! `SetAssignee { role }`, not `SetAssignee { user }`. The planner therefore
//! emits role-keyed effects and postconditions, and the executor needs a
//! runtime mapping from a declared workflow role to the Forge user that fills it
//! for this execution. [`ExecutionContext`] carries that mapping.
//!
//! Pull-request creation also needs runtime data that deliberately stays out of
//! the portable workflow spec: branch refs, title, body, labels, assignees, and
//! sometimes a per-work-item idempotency correlation key. A `CreatePullRequest`
//! effect may carry a static correlation key and may name the pull-request
//! artifact kind being created; the matching runtime key and concrete
//! [`temper_forge::CreatePullRequest`] input are still supplied here.
//!
//! The content-bearing `SetBody` and `AttachReview` effects share the same
//! shape: the effect declares only an optional correlation key, while the
//! agent-authored content (the new artifact body, or the review body) comes from
//! the workspace work product through the matching keyed runtime-input seam
//! supplied here. This mirrors `CreatePullRequest`: the spec stays portable and
//! the concrete authored text is bound where the runtime actually mutates.
//!
//! Keeping these bindings out of the spec and planner preserves the layering:
//! the spec and plan stay portable and backend-agnostic, while concrete
//! identity and branch choices are supplied where the runtime actually mutates a
//! backend. Missing bindings fail before any mutation, so a bad runtime context
//! can never partially apply a transition.

mod child;

pub use child::CreateIssuesChild;

use crate::ids::{RoleId, TransitionId};
use std::collections::{BTreeMap, BTreeSet};
use temper_forge::{CreatePullRequest, UserId};

/// Runtime context for a transition execution.
///
/// It resolves assignee roles to Forge users and supplies concrete
/// pull-request create inputs/correlation keys for `CreatePullRequest` effects.
/// Pull-request create inputs are keyed by transition id and a zero-based index
/// among that transition's create-PR effects; [`ExecutionContext::with_pull_request_create`]
/// is the convenience for the common single-create transition.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExecutionContext {
    assignees: BTreeMap<RoleId, UserId>,
    pull_request_creates: BTreeMap<(TransitionId, usize), CreatePullRequest>,
    satisfied_pull_request_creates: BTreeSet<(TransitionId, usize)>,
    pull_request_correlation_keys: BTreeMap<(TransitionId, usize), String>,
    set_body_inputs: BTreeMap<(TransitionId, usize), String>,
    set_body_correlation_keys: BTreeMap<(TransitionId, usize), String>,
    attach_review_inputs: BTreeMap<(TransitionId, usize), String>,
    attach_review_correlation_keys: BTreeMap<(TransitionId, usize), String>,
    create_issues_inputs: BTreeMap<(TransitionId, usize), Vec<CreateIssuesChild>>,
    create_issues_correlation_keys: BTreeMap<(TransitionId, usize), String>,
}

impl ExecutionContext {
    /// Creates an empty context with no role or create bindings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Binds a workflow role to the Forge user that fills it, returning `self`
    /// for chaining.
    pub fn with_assignee(mut self, role: RoleId, user: UserId) -> Self {
        self.assignees.insert(role, user);
        self
    }

    /// Binds a workflow role to the Forge user that fills it.
    pub fn set_assignee(&mut self, role: RoleId, user: UserId) -> &mut Self {
        self.assignees.insert(role, user);
        self
    }

    /// Resolves the Forge user bound to a role, if any.
    pub fn resolve_assignee(&self, role: &RoleId) -> Option<&UserId> {
        self.assignees.get(role)
    }

    /// Binds the first `CreatePullRequest` effect in `transition` to `input`,
    /// returning `self` for chaining.
    pub fn with_pull_request_create(
        self,
        transition: TransitionId,
        input: CreatePullRequest,
    ) -> Self {
        self.with_pull_request_create_at(transition, 0, input)
    }

    /// Binds the `index`-th `CreatePullRequest` effect in `transition` to
    /// `input`, returning `self` for chaining.
    pub fn with_pull_request_create_at(
        mut self,
        transition: TransitionId,
        index: usize,
        input: CreatePullRequest,
    ) -> Self {
        self.set_pull_request_create_at(transition, index, input);
        self
    }

    /// Binds the first `CreatePullRequest` effect in `transition` to `input`.
    pub fn set_pull_request_create(
        &mut self,
        transition: TransitionId,
        input: CreatePullRequest,
    ) -> &mut Self {
        self.set_pull_request_create_at(transition, 0, input)
    }

    /// Binds the `index`-th `CreatePullRequest` effect in `transition` to
    /// `input`.
    pub fn set_pull_request_create_at(
        &mut self,
        transition: TransitionId,
        index: usize,
        input: CreatePullRequest,
    ) -> &mut Self {
        self.pull_request_creates.insert((transition, index), input);
        self
    }

    /// Binds the `index`-th `CreatePullRequest` effect in `transition` to a
    /// runtime correlation key, returning `self` for chaining.
    pub fn with_pull_request_correlation_key_at(
        mut self,
        transition: TransitionId,
        index: usize,
        correlation_key: impl Into<String>,
    ) -> Self {
        self.set_pull_request_correlation_key_at(transition, index, correlation_key);
        self
    }

    /// Binds the `index`-th `CreatePullRequest` effect in `transition` to a
    /// runtime correlation key.
    pub fn set_pull_request_correlation_key_at(
        &mut self,
        transition: TransitionId,
        index: usize,
        correlation_key: impl Into<String>,
    ) -> &mut Self {
        self.pull_request_correlation_keys
            .insert((transition, index), correlation_key.into());
        self
    }

    /// Resolves the create input bound for the `index`-th `CreatePullRequest`
    /// effect in `transition`, if any.
    pub fn pull_request_create(
        &self,
        transition: &TransitionId,
        index: usize,
    ) -> Option<&CreatePullRequest> {
        self.pull_request_creates.get(&(transition.clone(), index))
    }

    /// Marks the `index`-th `CreatePullRequest` effect as already satisfied.
    /// This lets a runtime binding commit the remaining transition effects when
    /// source work was performed directly on the pull request's target branch.
    pub fn set_pull_request_create_satisfied_at(
        &mut self,
        transition: TransitionId,
        index: usize,
    ) -> &mut Self {
        self.satisfied_pull_request_creates
            .insert((transition, index));
        self
    }

    /// Reports whether the runtime has already satisfied the indexed create.
    pub fn pull_request_create_is_satisfied(
        &self,
        transition: &TransitionId,
        index: usize,
    ) -> bool {
        self.satisfied_pull_request_creates
            .contains(&(transition.clone(), index))
    }

    /// Resolves the runtime correlation key bound for the `index`-th
    /// `CreatePullRequest` effect in `transition`, if any.
    pub fn pull_request_correlation_key(
        &self,
        transition: &TransitionId,
        index: usize,
    ) -> Option<&str> {
        self.pull_request_correlation_keys
            .get(&(transition.clone(), index))
            .map(String::as_str)
    }

    /// Binds the `index`-th `SetBody` effect in `transition` to the
    /// agent-authored body text, returning `self` for chaining.
    pub fn with_set_body_at(
        mut self,
        transition: TransitionId,
        index: usize,
        body: impl Into<String>,
    ) -> Self {
        self.set_set_body_at(transition, index, body);
        self
    }

    /// Binds the first `SetBody` effect in `transition` to the agent-authored
    /// body text.
    pub fn set_set_body(&mut self, transition: TransitionId, body: impl Into<String>) -> &mut Self {
        self.set_set_body_at(transition, 0, body)
    }

    /// Binds the `index`-th `SetBody` effect in `transition` to the
    /// agent-authored body text.
    pub fn set_set_body_at(
        &mut self,
        transition: TransitionId,
        index: usize,
        body: impl Into<String>,
    ) -> &mut Self {
        self.set_body_inputs
            .insert((transition, index), body.into());
        self
    }

    /// Binds the `index`-th `SetBody` effect in `transition` to a runtime
    /// correlation key, returning `self` for chaining.
    pub fn with_set_body_correlation_key_at(
        mut self,
        transition: TransitionId,
        index: usize,
        correlation_key: impl Into<String>,
    ) -> Self {
        self.set_set_body_correlation_key_at(transition, index, correlation_key);
        self
    }

    /// Binds the `index`-th `SetBody` effect in `transition` to a runtime
    /// correlation key.
    pub fn set_set_body_correlation_key_at(
        &mut self,
        transition: TransitionId,
        index: usize,
        correlation_key: impl Into<String>,
    ) -> &mut Self {
        self.set_body_correlation_keys
            .insert((transition, index), correlation_key.into());
        self
    }

    /// Resolves the body bound for the `index`-th `SetBody` effect in
    /// `transition`, if any.
    pub fn set_body(&self, transition: &TransitionId, index: usize) -> Option<&str> {
        self.set_body_inputs
            .get(&(transition.clone(), index))
            .map(String::as_str)
    }

    /// Resolves the runtime correlation key bound for the `index`-th `SetBody`
    /// effect in `transition`, if any.
    pub fn set_body_correlation_key(
        &self,
        transition: &TransitionId,
        index: usize,
    ) -> Option<&str> {
        self.set_body_correlation_keys
            .get(&(transition.clone(), index))
            .map(String::as_str)
    }

    /// Binds the `index`-th `AttachReview` effect in `transition` to the
    /// agent-authored review body, returning `self` for chaining.
    pub fn with_attach_review_at(
        mut self,
        transition: TransitionId,
        index: usize,
        body: impl Into<String>,
    ) -> Self {
        self.set_attach_review_at(transition, index, body);
        self
    }

    /// Binds the first `AttachReview` effect in `transition` to the
    /// agent-authored review body.
    pub fn set_attach_review(
        &mut self,
        transition: TransitionId,
        body: impl Into<String>,
    ) -> &mut Self {
        self.set_attach_review_at(transition, 0, body)
    }

    /// Binds the `index`-th `AttachReview` effect in `transition` to the
    /// agent-authored review body.
    pub fn set_attach_review_at(
        &mut self,
        transition: TransitionId,
        index: usize,
        body: impl Into<String>,
    ) -> &mut Self {
        self.attach_review_inputs
            .insert((transition, index), body.into());
        self
    }

    /// Binds the `index`-th `AttachReview` effect in `transition` to a runtime
    /// correlation key, returning `self` for chaining.
    pub fn with_attach_review_correlation_key_at(
        mut self,
        transition: TransitionId,
        index: usize,
        correlation_key: impl Into<String>,
    ) -> Self {
        self.set_attach_review_correlation_key_at(transition, index, correlation_key);
        self
    }

    /// Binds the `index`-th `AttachReview` effect in `transition` to a runtime
    /// correlation key.
    pub fn set_attach_review_correlation_key_at(
        &mut self,
        transition: TransitionId,
        index: usize,
        correlation_key: impl Into<String>,
    ) -> &mut Self {
        self.attach_review_correlation_keys
            .insert((transition, index), correlation_key.into());
        self
    }

    /// Resolves the review body bound for the `index`-th `AttachReview` effect
    /// in `transition`, if any.
    pub fn attach_review(&self, transition: &TransitionId, index: usize) -> Option<&str> {
        self.attach_review_inputs
            .get(&(transition.clone(), index))
            .map(String::as_str)
    }

    /// Resolves the runtime correlation key bound for the `index`-th
    /// `AttachReview` effect in `transition`, if any.
    pub fn attach_review_correlation_key(
        &self,
        transition: &TransitionId,
        index: usize,
    ) -> Option<&str> {
        self.attach_review_correlation_keys
            .get(&(transition.clone(), index))
            .map(String::as_str)
    }

    /// Binds the `index`-th `CreateIssues` effect in `transition` to the
    /// workspace-authored children, returning `self` for chaining.
    pub fn with_create_issues_at(
        mut self,
        transition: TransitionId,
        index: usize,
        children: impl IntoIterator<Item = CreateIssuesChild>,
    ) -> Self {
        self.set_create_issues_at(transition, index, children);
        self
    }

    /// Binds the first `CreateIssues` effect in `transition` to the
    /// workspace-authored children.
    pub fn set_create_issues(
        &mut self,
        transition: TransitionId,
        children: impl IntoIterator<Item = CreateIssuesChild>,
    ) -> &mut Self {
        self.set_create_issues_at(transition, 0, children)
    }

    /// Binds the `index`-th `CreateIssues` effect in `transition` to the
    /// workspace-authored children.
    pub fn set_create_issues_at(
        &mut self,
        transition: TransitionId,
        index: usize,
        children: impl IntoIterator<Item = CreateIssuesChild>,
    ) -> &mut Self {
        self.create_issues_inputs
            .insert((transition, index), children.into_iter().collect());
        self
    }

    /// Binds the `index`-th `CreateIssues` effect in `transition` to a runtime
    /// base correlation key, returning `self` for chaining.
    pub fn with_create_issues_correlation_key_at(
        mut self,
        transition: TransitionId,
        index: usize,
        correlation_key: impl Into<String>,
    ) -> Self {
        self.set_create_issues_correlation_key_at(transition, index, correlation_key);
        self
    }

    /// Binds the `index`-th `CreateIssues` effect in `transition` to a runtime
    /// base correlation key.
    pub fn set_create_issues_correlation_key_at(
        &mut self,
        transition: TransitionId,
        index: usize,
        correlation_key: impl Into<String>,
    ) -> &mut Self {
        self.create_issues_correlation_keys
            .insert((transition, index), correlation_key.into());
        self
    }

    /// Resolves the children bound for the `index`-th `CreateIssues` effect in
    /// `transition`, if any.
    pub fn create_issues(
        &self,
        transition: &TransitionId,
        index: usize,
    ) -> Option<&[CreateIssuesChild]> {
        self.create_issues_inputs
            .get(&(transition.clone(), index))
            .map(Vec::as_slice)
    }

    /// Resolves the runtime base correlation key bound for the `index`-th
    /// `CreateIssues` effect in `transition`, if any.
    pub fn create_issues_correlation_key(
        &self,
        transition: &TransitionId,
        index: usize,
    ) -> Option<&str> {
        self.create_issues_correlation_keys
            .get(&(transition.clone(), index))
            .map(String::as_str)
    }
}
