use crate::ids::{CommentId, LabelId, RepositoryId, UserId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Optimistic-concurrency version token for an issue or pull request.
///
/// Backends increment an artifact's version on every successful mutation of its
/// record. A caller that captures the version at read time can pass it back as
/// an [`UpdateIssue::expected_version`](crate::UpdateIssue::expected_version) /
/// [`UpdatePullRequest::expected_version`](crate::UpdatePullRequest::expected_version)
/// precondition: the conditional update applies only if the stored version
/// still matches, and otherwise fails with
/// [`ForgeError::Conflict`](crate::ForgeError::Conflict). This is the portable
/// optimistic-concurrency primitive (see ADR 0013); a real forge maps it onto an
/// `ETag`/`If-Match` pair or an equivalent conditional write.
///
/// The token is a dedicated monotonic counter, not a timestamp. Reusing
/// `updated_at` would collide whenever two mutations share a clock value (the
/// reference backends advance the clock by a whole second per write), which
/// would silently defeat the precondition. A counter advances on every write, so
/// no two successive versions ever coincide.
#[derive(Copy, Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct Version(u64);

impl Version {
    /// The version assigned to a freshly created artifact.
    pub const INITIAL: Version = Version(1);

    /// Creates a version token from a raw counter value.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the raw counter value.
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Returns the next version, saturating at the maximum representable value.
    pub const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

impl Default for Version {
    /// The default token is [`Version::INITIAL`], so a record deserialized from a
    /// pre-versioning store reads as the initial version rather than failing.
    fn default() -> Self {
        Version::INITIAL
    }
}

impl fmt::Display for Version {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// User account known to a Forge backend.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct User {
    pub id: UserId,
    pub handle: String,
    pub display_name: Option<String>,
    pub email: Option<String>,
}

/// Human-facing owner/name repository lookup key.
///
/// A repository path is convenient for user input and provider URLs, but it is
/// not stable identity. Store `RepositoryId` for durable synchronization.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RepositoryPath {
    pub owner: String,
    pub name: String,
}

impl RepositoryPath {
    /// Creates a repository path from owner and repository name values.
    pub fn new(owner: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            owner: owner.into(),
            name: name.into(),
        }
    }
}

/// Repository containing source code and collaboration artifacts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Repository {
    pub id: RepositoryId,
    pub owner: String,
    pub name: String,
    pub default_branch: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Input used to create a repository.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CreateRepository {
    pub owner: String,
    pub name: String,
    pub default_branch: String,
    pub description: Option<String>,
}

/// Label metadata scoped to a repository.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Label {
    pub id: LabelId,
    pub repo_id: RepositoryId,
    pub name: String,
    pub color: Option<String>,
    pub description: Option<String>,
}

/// Input used to create or update a label.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UpsertLabel {
    pub name: String,
    pub color: Option<String>,
    pub description: Option<String>,
}

/// Comment on an issue or pull request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Comment {
    pub id: CommentId,
    pub author_id: UserId,
    pub body: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Input used to add a comment.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CreateComment {
    pub body: String,
}

/// Reference to a branch in a repository.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BranchRef {
    pub repository_id: RepositoryId,
    pub branch: String,
}
