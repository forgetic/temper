//! Typed identifiers for workflow specification symbols.
//!
//! A workflow specification declares symbols (roles, labels, artifact kinds,
//! state dimensions, states, queues, transitions, and gates) by string id.
//! These newtypes keep those ids distinct in the validated model so that, for
//! example, a [`RoleId`] cannot be passed where a [`QueueId`] is expected.
//!
//! The raw, serde-loaded spec in [`crate::spec`] uses plain `String` ids;
//! validation in [`crate::validate`] converts them into these typed ids while
//! checking for duplicates and undeclared references.

use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! define_spec_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Creates a new identifier from a spec-provided value.
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            /// Returns the identifier as a string slice.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self::new(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self::new(value)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

define_spec_id!(RoleId, "Identifier for a workflow role.");
define_spec_id!(
    LabelId,
    "Identifier for a workflow label projection (a Forge label name)."
);
define_spec_id!(ArtifactKindId, "Identifier for a workflow artifact kind.");
define_spec_id!(
    StateDimensionId,
    "Identifier for a workflow state dimension."
);
define_spec_id!(StateId, "Identifier for a state within a state dimension.");
define_spec_id!(QueueId, "Identifier for a workflow queue.");
define_spec_id!(TransitionId, "Identifier for a workflow transition.");
define_spec_id!(GateId, "Identifier for a workflow gate.");
define_spec_id!(
    ValidationBindingId,
    "Identifier for a workflow validation binding."
);
define_spec_id!(
    ExternalToolId,
    "Identifier for a user-declared external tool."
);
define_spec_id!(
    VerdictId,
    "Identifier for a workflow verdict token. Verdict ids are workflow vocabulary; the engine treats them as opaque tokens and only validates, at compile time, that each maps to a transition that is legal for the action's artifact/role."
);

impl VerdictId {
    /// The built-in verdict the engine derives from a merge-conflict execution
    /// outcome. The `on_merge_conflict` queue-automation sugar desugars to an
    /// outcome keyed by this id, making it the first instance of verdict routing.
    pub fn merge_conflict() -> Self {
        Self::new("merge_conflict")
    }
}
