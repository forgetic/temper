use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! define_string_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Creates a new identifier from a backend-provided stable value.
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

define_string_id!(RepositoryId, "Stable backend identifier for a repository.");
define_string_id!(UserId, "Stable backend identifier for a user.");
define_string_id!(IssueId, "Stable backend identifier for an issue.");
define_string_id!(
    PullRequestId,
    "Stable backend identifier for a pull request."
);
define_string_id!(CommentId, "Stable backend identifier for a comment.");
define_string_id!(LabelId, "Stable backend identifier for a label.");
define_string_id!(CiJobId, "Stable backend identifier for a CI job.");

/// Human-facing repository-scoped item number.
///
/// Many Forge systems expose issue and pull-request numbers that are easier for
/// humans to use than opaque backend identifiers. The Forge model keeps both.
#[derive(Copy, Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ItemNumber(u64);

impl ItemNumber {
    /// Creates an item number.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the numeric item number.
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl From<u64> for ItemNumber {
    fn from(value: u64) -> Self {
        Self::new(value)
    }
}

impl fmt::Display for ItemNumber {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}
