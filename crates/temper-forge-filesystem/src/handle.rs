use crate::FilesystemForge;
use crate::metadata::Metadata;
use std::path::{Path, PathBuf};
use temper_forge::User;

impl FilesystemForge {
    /// Creates a backend rooted at `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            current_user: None,
        }
    }

    /// Returns a handle rooted at the same store that acts as `user`.
    ///
    /// The identity override lives on the handle, not in `metadata.json`, so
    /// clones of the returned handle preserve the override while other handles
    /// can act as different users.
    pub fn as_user(&self, user: User) -> Self {
        Self {
            root: self.root.clone(),
            current_user: Some(user),
        }
    }

    pub(crate) fn effective_user(&self, metadata: &Metadata) -> User {
        self.current_user
            .clone()
            .unwrap_or_else(|| metadata.current_user.clone())
    }

    /// Returns the filesystem root used by this backend.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the directory used to store repository records.
    pub fn repositories_dir(&self) -> PathBuf {
        self.root.join("repositories")
    }
}
