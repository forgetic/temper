//! Artifact target mapping.
//!
//! An artifact kind is a logical work item that maps to exactly one kind of
//! Forge artifact. [`ArtifactTarget`] names which Forge artifact type a kind is
//! projected onto so the classifier can decide whether to read a Forge issue or
//! a Forge pull request.
//!
//! This stays provider-neutral: it names the abstract Forge artifact type, not
//! a backend-specific concept.

use harness_forge::{ItemNumber, RepositoryId};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// The Forge artifact type a workflow artifact kind maps to.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactTarget {
    /// The artifact kind is represented by a Forge issue.
    #[default]
    Issue,
    /// The artifact kind is represented by a Forge pull request.
    PullRequest,
}

impl fmt::Display for ArtifactTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            ArtifactTarget::Issue => "issue",
            ArtifactTarget::PullRequest => "pull request",
        };
        formatter.write_str(text)
    }
}

/// Reference to a Forge issue or pull request by repository and item number.
///
/// `repository_id == None` is the backward-compatible same-repository shorthand
/// used by existing metadata and same-repo native dependency links. Runtime code
/// can resolve it against the source artifact's repository when it needs a fully
/// qualified `(RepositoryId, ItemNumber)` pair.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ArtifactRef {
    /// Target repository. `None` means the source artifact's own repository.
    pub repository_id: Option<RepositoryId>,
    /// Human-facing issue or pull-request number inside the target repository.
    pub number: ItemNumber,
}

impl ArtifactRef {
    /// Creates a same-repository reference.
    pub const fn same_repo(number: ItemNumber) -> Self {
        Self {
            repository_id: None,
            number,
        }
    }

    /// Creates an explicit repository-qualified reference.
    pub fn in_repo(repository_id: impl Into<RepositoryId>, number: ItemNumber) -> Self {
        Self {
            repository_id: Some(repository_id.into()),
            number,
        }
    }

    /// Returns whether this reference uses the same-repository shorthand.
    pub fn is_same_repo(&self) -> bool {
        self.repository_id.is_none()
    }

    /// Returns whether this reference resolves inside `repository_id`.
    pub fn is_in_repository(&self, repository_id: &RepositoryId) -> bool {
        match &self.repository_id {
            Some(target) => target == repository_id,
            None => true,
        }
    }

    /// Resolves the same-repository shorthand against `repository_id`.
    pub fn resolved_repository(&self, repository_id: &RepositoryId) -> RepositoryId {
        self.repository_id
            .clone()
            .unwrap_or_else(|| repository_id.clone())
    }
}

impl From<ItemNumber> for ArtifactRef {
    fn from(number: ItemNumber) -> Self {
        Self::same_repo(number)
    }
}

impl From<&ArtifactRef> for ArtifactRef {
    fn from(reference: &ArtifactRef) -> Self {
        reference.clone()
    }
}

impl Serialize for ArtifactRef {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match &self.repository_id {
            Some(repository_id) => ArtifactRefObject {
                repository_id: repository_id.clone(),
                number: self.number,
            }
            .serialize(serializer),
            None => self.number.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for ArtifactRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Number(ItemNumber),
            Object(ArtifactRefObject),
        }

        match Repr::deserialize(deserializer)? {
            Repr::Number(number) => Ok(Self::same_repo(number)),
            Repr::Object(object) => Ok(Self::in_repo(object.repository_id, object.number)),
        }
    }
}

#[derive(Deserialize, Serialize)]
struct ArtifactRefObject {
    repository_id: RepositoryId,
    number: ItemNumber,
}
