//! Runtime bindings the pure planner cannot know (Phase 9b).
//!
//! A workflow spec references *roles*, never concrete Forge users: an effect is
//! `SetAssignee { role }`, not `SetAssignee { user }`. The planner therefore
//! emits role-keyed effects and postconditions, and the executor needs a
//! runtime mapping from a declared workflow role to the Forge user that fills it
//! for this execution. [`ExecutionContext`] carries that mapping.
//!
//! Keeping role→user resolution out of the spec and the planner preserves the
//! layering: the spec and plan stay portable and backend-agnostic, while the
//! concrete identity binding is supplied where the runtime actually mutates a
//! backend. An execution that plans a `SetAssignee`/`RemoveAssignee` effect for
//! a role with no binding fails with
//! [`ExecutionError::UnresolvedAssignee`](crate::execute::ExecutionError::UnresolvedAssignee)
//! *before* any mutation, so a missing binding can never partially apply.

use crate::ids::RoleId;
use harness_forge::UserId;
use std::collections::BTreeMap;

/// Runtime context for a transition execution.
///
/// Today it only resolves assignee roles to Forge users; future runtime
/// bindings (for example the worker identity used to stamp create requests) can
/// be added here without changing the planner or the spec.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExecutionContext {
    assignees: BTreeMap<RoleId, UserId>,
}

impl ExecutionContext {
    /// Creates an empty context with no role bindings.
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
}
