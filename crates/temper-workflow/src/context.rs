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
//! effect may carry a static correlation key; when it does not, the matching
//! runtime key and [`temper_forge::CreatePullRequest`] input are supplied here.
//!
//! Keeping these bindings out of the spec and planner preserves the layering:
//! the spec and plan stay portable and backend-agnostic, while concrete
//! identity and branch choices are supplied where the runtime actually mutates a
//! backend. Missing bindings fail before any mutation, so a bad runtime context
//! can never partially apply a transition.

use crate::ids::{RoleId, TransitionId};
use std::collections::BTreeMap;
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
    pull_request_correlation_keys: BTreeMap<(TransitionId, usize), String>,
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
}
