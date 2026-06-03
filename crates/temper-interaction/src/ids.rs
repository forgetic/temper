//! Typed ids for user-defined interaction profile specifications.
//!
//! Raw interaction specs use plain strings so validation can collect duplicate,
//! malformed, and dangling-reference diagnostics in one pass. The validated
//! model converts those strings into distinct id types after the deterministic
//! slug rule has been checked.

use std::fmt;

macro_rules! define_interaction_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            pub(crate) fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            /// Returns the identifier as a string slice.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

define_interaction_id!(
    InteractionSpecId,
    "Identifier for an interaction spec document."
);
define_interaction_id!(
    ResponderId,
    "Identifier for an interactive responder declaration."
);
define_interaction_id!(CommandId, "Identifier for a transport command declaration.");
define_interaction_id!(
    AcceptanceActionId,
    "Identifier for an accepted-proposal action declaration."
);
